//! The loading screen: the world you are travelling to, turning slowly, with its name, the
//! place you arrive at and a few facts, a progress bar with the current loading stage, a
//! spinner, and tips that fade from one to the next. When loading finishes it waits for
//! Enter or a click (unless `hold` is false), then closes and sends `game.loading_done`.
//!
//! ```ignore
//! // The game drives the bar through a data path...
//! ui::show(world, "loading", &json!({"planet": "kell", "place": "Wreck Field", "progress": "loading.progress"}))?;
//! world.resource_mut::<UiData>().set("loading.progress", 0.4);
//! // ...or the screen fills it by itself over `simulate_s` seconds (demos, the tour).
//! ui::show(world, "loading", &json!({"planet": "veyra", "simulate_s": 6.0}))?;
//! ```

use bevy_ecs::message::MessageCursor;
use bevy_ecs::prelude::*;
use engine_core::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::f32::consts::TAU;

use crate::color::Color;
use crate::data::UiData;
use crate::draw::{DrawCtx, Painters};
use crate::el::El;
use crate::input::UiEvent;
use crate::map::{MapData, Planet};
use crate::node::*;
use crate::screens;
use crate::style::{Corners, Fill, Justify, Pick, Position};
use crate::theme::Theme;

/// Tips shown when the caller gives none: half lore, half how to play.
pub const DEFAULT_TIPS: &[&str] = &[
    "The relay towers still hum on Veyra's northern ridges. Nobody alive remembers who built them.",
    "Hold the quick-select wheel to slow the world while you pick a weapon or a spell.",
    "Traders pay more for goods they lack. Check the shop's sell column before you haul ore across a system.",
    "A gold pin on the map follows the tracked quest. Track another in the journal to move it.",
    "Kell's wreck fields are cold enough to freeze a careless pilot's fuel lines. Pack a fuel cell.",
    "Recipes you have not learned show their ingredients, so you know what to gather first.",
    "Autosaves are made when you land, dock or rest. Quick save with F5 at any time.",
    "Thessaly's great storm has turned for three hundred years. Skimmers ride its outer bands for fuel.",
];

/// Stage names by progress: each shows from its threshold to the next one.
const STAGES: &[(f32, &str)] = &[
    (0.0, "Plotting the descent"),
    (0.14, "Streaming terrain"),
    (0.34, "Growing forests and meadows"),
    (0.52, "Filling rivers and seas"),
    (0.68, "Raising the settlements"),
    (0.84, "Waking the locals"),
    (1.0, "Ready"),
];

/// Seconds each tip stays up, and the cross-fade between two.
const TIP_SECS: f32 = 6.5;
const TIP_FADE: f32 = 0.6;

/// The stage name shown at `progress` (0..1).
pub fn stage(progress: f32) -> &'static str {
    STAGES
        .iter()
        .rev()
        .find(|(at, _)| progress >= *at)
        .map_or(STAGES[0].1, |(_, s)| s)
}

/// State of the painted part of the loading screen (on the node with `paint:
/// "loading_backdrop"`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoadingView {
    /// Real seconds since the screen opened.
    pub time: f32,
    /// Target progress 0..1 (from the data path or the simulation).
    pub progress: f32,
    /// What the bar shows: eases toward `progress`.
    pub shown: f32,
    /// The planet (MapData id) in the backdrop.
    pub planet: String,
    pub tips: Vec<String>,
    /// Index of the first tip shown.
    pub tip_from: usize,
    /// Seconds since loading finished (0 until then); drives the "ready" flash.
    pub ready_for: f32,
}

impl Default for LoadingView {
    fn default() -> Self {
        Self {
            time: 0.0,
            progress: 0.0,
            shown: 0.0,
            planet: String::new(),
            tips: Vec::new(),
            tip_from: 0,
            ready_for: 0.0,
        }
    }
}

impl LoadingView {
    /// The tip up at `time`, and its opacity in the cross-fade.
    pub fn tip(&self) -> Option<(&str, f32)> {
        if self.tips.is_empty() {
            return None;
        }
        let n = (self.time / TIP_SECS) as usize;
        let i = (self.tip_from + n) % self.tips.len();
        let t = self.time % TIP_SECS;
        // The first tip is up at once; later ones fade in.
        let fade_in = if n == 0 { 1.0 } else { t / TIP_FADE };
        let a = fade_in.min((TIP_SECS - t) / TIP_FADE).clamp(0.0, 1.0);
        Some((self.tips[i].as_str(), a))
    }

    pub fn ready(&self) -> bool {
        self.progress >= 1.0 && self.shown >= 0.999
    }
}

fn paint_backdrop(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(v) = world.get::<LoadingView>(e).cloned() else {
        return;
    };
    let planet = world
        .get_resource::<MapData>()
        .and_then(|m| m.planet(&v.planet).map(|(_, p)| p.clone()))
        .unwrap_or_default();
    let accent = world.resource::<Theme>().palette.accent;
    let px = ctx.px;
    let t = v.time;
    let r = ctx.rect;
    let (w, h) = (r.w, r.h);
    let at = |x: f32, y: f32| Vec2::new(r.x + w * x, r.y + h * y);

    // Space, drifting stars and a wash of the planet's air color.
    ctx.rect_fill(
        r,
        Fill::Linear {
            from: Color::hex(0x020309),
            to: Color::hex(0x060c18),
            angle_deg: 35.0,
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    ctx.starfield(r, Vec2::new(t * 6.0, t * 1.5), 1.0, 3.7, 0.0);
    let look = planet.look();
    let air = Color::linear(
        look.atmosphere[0],
        look.atmosphere[1],
        look.atmosphere[2],
        1.0,
    );
    let air = if look.atmosphere[3] > 0.0 {
        air
    } else {
        Color::hex(0x8a9ab8)
    };
    // A ringed world sits smaller and further right so its rings clear the text.
    let (pc, pr) = if planet.rings {
        (at(0.73, 0.45), h * 0.25)
    } else {
        (at(0.70, 0.47), h * 0.36)
    };
    ctx.glow(pc, pr * 2.2, air.with_alpha(0.07), 1.1);
    ctx.glow(pc, pr * 1.3, air.with_alpha(0.16), 1.6);
    // Sunlight from the upper left, the night side to the lower right.
    let (az, el) = (-0.95, 0.42);
    if planet.rings {
        crate::map::ring_half(ctx, pc, pr, true);
    }
    let halo = if look.atmosphere[3] > 0.0 { 0.07 } else { 0.0 };
    ctx.planet(pc, pr, look, planet.seed + t * 0.035, 0.32, az, el, halo);
    if planet.rings {
        crate::map::ring_half(ctx, pc, pr, false);
    }
    // A thin scanning frame around the globe.
    ctx.ring(
        pc,
        Vec2::splat(pr * 1.17),
        1.0 * px,
        Color::WHITE.with_alpha(0.10),
        t * 0.05,
        TAU,
        96.0,
    );
    ctx.ring(
        pc,
        Vec2::splat(pr * 1.17),
        2.0 * px,
        accent.with_alpha(0.55),
        t * 0.4,
        TAU * 0.08,
        0.0,
    );
    // Darker on the left for the text, and along the bottom for the bar.
    ctx.rect_fill(
        r,
        Fill::Linear {
            from: Color::hexa(0x02040ae0),
            to: Color::hexa(0x02040a00),
            angle_deg: 0.0,
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    let band = Rect::new(r.x, r.y + h * 0.66, w, h * 0.34);
    ctx.rect_fill(
        band,
        Fill::Linear {
            from: Color::hexa(0x02040a00),
            to: Color::hexa(0x02040af0),
            angle_deg: 90.0,
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    ctx.rect_fill(
        r,
        Fill::Radial {
            inner: Color::hexa(0x00000000),
            outer: Color::hexa(0x00000080),
        },
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );

    // The bar: a dark track, the fill, a bright head and the stage above it.
    let left = r.x + 140.0 * px;
    let right = r.x + w - 220.0 * px;
    let y = r.y + h - 104.0 * px;
    let bh = 6.0 * px;
    let track = Rect::new(left, y, right - left, bh);
    ctx.rect(
        track,
        Color::WHITE.with_alpha(0.08),
        bh * 0.5,
        0.0,
        Color::TRANSPARENT,
    );
    let p = v.shown.clamp(0.0, 1.0);
    let ready = v.ready();
    let flash = if ready {
        (1.0 - v.ready_for / 0.8).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if p > 0.0 {
        let fw = (track.w * p).max(bh);
        let fill = Rect::new(left, y, fw, bh);
        ctx.rect_fill(
            fill,
            Fill::Linear {
                from: accent.scale(0.7),
                to: accent.lerp(Color::WHITE, 0.35 + flash * 0.5),
                angle_deg: 0.0,
            },
            [bh * 0.5; 4],
            0.0,
            Color::TRANSPARENT,
        );
        // A sheen running along the filled part.
        let sx = left + ((t * 0.45) % 1.0) * fw;
        ctx.with_clip(fill, |ctx| {
            ctx.glow(
                Vec2::new(sx, y + bh * 0.5),
                40.0 * px,
                Color::WHITE.with_alpha(0.22),
                2.0,
            );
        });
        let head = Vec2::new(left + fw - bh * 0.5, y + bh * 0.5);
        ctx.glow(head, 26.0 * px, accent.with_alpha(0.45 + flash * 0.4), 2.2);
        ctx.glow(head, 6.0 * px, Color::WHITE.with_alpha(0.9), 2.0);
    }
    if flash > 0.0 {
        ctx.glow(
            Vec2::new((left + right) * 0.5, y + bh * 0.5),
            (right - left) * 0.55,
            accent.with_alpha(0.10 * flash),
            1.6,
        );
    }
    let label_y = y - 14.0 * px;
    let stage_text = stage(v.progress.min(p + 0.001));
    let dots = if ready {
        String::new()
    } else {
        ".".repeat(1 + (t * 2.0) as usize % 3)
    };
    ctx.text(
        &format!("{}{}", stage_text.to_uppercase(), dots),
        13.0,
        700,
        if ready { accent } else { Color::hex(0xc8d0e0) },
        Vec2::new(left, label_y),
        Vec2::new(0.0, 1.0),
    );
    if ready {
        let pulse = 0.55 + 0.45 * (v.ready_for * 3.2).cos().abs();
        ctx.text(
            "PRESS ENTER OR CLICK TO CONTINUE",
            13.0,
            700,
            Color::WHITE.with_alpha(pulse),
            Vec2::new(right, label_y),
            Vec2::new(1.0, 1.0),
        );
    } else {
        ctx.text(
            &format!("{:.0}%", p * 100.0),
            13.0,
            700,
            Color::hex(0x8a94a8),
            Vec2::new(right, label_y),
            Vec2::new(1.0, 1.0),
        );
    }

    // The tip under the bar, fading between tips.
    if let Some((tip, a)) = v.tip() {
        let ty = y + 30.0 * px;
        let tag = ctx.text(
            "TIP",
            12.0,
            800,
            accent.with_alpha(a),
            Vec2::new(left, ty),
            Vec2::new(0.0, 0.0),
        );
        ctx.text(
            tip,
            15.0,
            500,
            Color::hex(0xb4bccb).with_alpha(a),
            Vec2::new(tag.x + tag.w + 12.0 * px, ty - 1.5 * px),
            Vec2::new(0.0, 0.0),
        );
    }

    // The spinner at the right end of the bar; a check once ready.
    let sc = Vec2::new(r.x + w - 140.0 * px, y + bh * 0.5);
    let sr = 22.0 * px;
    ctx.ring(
        sc,
        Vec2::splat(sr),
        2.0 * px,
        Color::WHITE.with_alpha(0.10),
        0.0,
        TAU,
        0.0,
    );
    if ready {
        let k = (v.ready_for / 0.35).clamp(0.0, 1.0);
        ctx.ring(
            sc,
            Vec2::splat(sr),
            2.5 * px,
            accent,
            -TAU * 0.25,
            TAU * k,
            0.0,
        );
        ctx.glow(sc, sr * 1.8, accent.with_alpha(0.18), 1.6);
        let s = 18.0 * px * k;
        ctx.icon(
            "ui_check",
            Rect::new(sc.x - s * 0.5, sc.y - s * 0.5, s, s),
            accent,
        );
    } else {
        let spin = t * 4.2;
        let sweep = TAU * (0.18 + 0.14 * (t * 2.1).sin().abs());
        ctx.ring(sc, Vec2::splat(sr), 2.5 * px, accent, spin, sweep, 0.0);
        ctx.ring(
            sc,
            Vec2::splat(sr * 0.62),
            1.5 * px,
            Color::WHITE.with_alpha(0.45),
            -spin * 0.7,
            TAU * 0.22,
            0.0,
        );
        ctx.glow(sc, sr * 1.6, accent.with_alpha(0.08), 1.6);
    }
}

pub fn add_painters(p: &mut Painters) {
    p.add("loading_backdrop", paint_backdrop);
}

/// The open loading screen (on its root).
#[derive(Component, Debug, Clone, Default)]
pub struct LoadingScreen {
    /// Data path the game writes progress to (0..1); empty = simulate.
    pub progress_path: String,
    /// Seconds a simulated load takes, and where it starts (0..1).
    pub simulate_s: f32,
    pub start: f32,
    /// Wait for Enter or a click when done.
    pub hold: bool,
}

fn fact(icon: &str, label: &str, value: &str, theme: &Theme) -> El {
    let p = &theme.palette;
    El::new()
        .class("col")
        .with_style(|s| s.gap(3.0).shrink(0.0))
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(6.0))
                .child(
                    El::icon(icon)
                        .with_style(|s| s.size(13.0, 13.0))
                        .icon_tint(p.accent),
                )
                .child(
                    El::text(label.to_uppercase())
                        .text_style(|t| t.bold().nowrap())
                        .with_style(|s| s.font_size(11.0).color(p.text_faint)),
                ),
        )
        .child(
            El::text(value)
                .text_style(|t| t.semibold().nowrap())
                .with_style(|s| s.font_size(16.0).color(Color::WHITE)),
        )
}

fn build(world: &World, planet: &Planet, system: &str, place: &str, view: &LoadingView) -> El {
    let theme = world.resource::<Theme>();
    let p = &theme.palette;
    let mut facts = El::new()
        .class("row")
        .with_style(|s| s.gap(34.0).margin([10.0, 0.0, 0.0, 0.0]));
    facts = facts.child(fact(
        "planet",
        "Gravity",
        &format!("{:.2} g", planet.gravity_g),
        theme,
    ));
    if !planet.atmosphere.is_empty() {
        facts = facts.child(fact("compass", "Air", &planet.atmosphere, theme));
    }
    if !planet.population.is_empty() {
        facts = facts.child(fact("marker_city", "People", &planet.population, theme));
    }
    if planet.moons > 0 {
        facts = facts.child(fact("ui_galaxy", "Moons", &planet.moons.to_string(), theme));
    }
    let where_line = match (place.is_empty(), system.is_empty()) {
        (false, false) => format!("{place}  ·  {system} system"),
        (false, true) => place.to_string(),
        (true, false) => format!("{system} system"),
        (true, true) => String::new(),
    };
    let info = El::new()
        .class("col")
        .with_style(|s| {
            s.position(Position::Absolute)
                .left(140.0)
                .top(0.0)
                .bottom(170.0)
                .justify(Justify::Center)
                .gap(10.0)
                .width(560.0)
        })
        .child(
            El::text("NOW ARRIVING")
                .text_style(|t| t.bold().nowrap())
                .with_style(|s| s.font_size(13.0).color(p.accent)),
        )
        .child(
            El::text(planet.name.to_uppercase())
                .text_style(|t| t.bold().nowrap().shadow())
                .with_style(|s| s.font_size(92.0).color(Color::WHITE)),
        )
        .child(
            El::new()
                .with_style(|s| {
                    s.size(140.0, 3.0)
                        .radius(Corners::all(2.0))
                        .bg(p.accent)
                        .shadow(crate::style::Shadow::glow(p.accent.fade(0.6), 12.0))
                })
                .pick_ignore(),
        )
        .child_opt((!where_line.is_empty()).then(|| {
            El::text(where_line)
                .text_style(|t| t.semibold().nowrap().shadow())
                .with_style(|s| {
                    s.font_size(21.0)
                        .color(Color::hex(0xe4e8f0))
                        .margin([4.0, 0.0, 0.0, 0.0])
                })
        }))
        .child_opt((!planet.description.is_empty()).then(|| {
            El::text(planet.description.clone())
                .text_style(|t| t.shadow())
                .with_style(|s| s.font_size(17.0).color(p.text_dim).max_width(520.0))
        }))
        .child(facts);
    El::new()
        .class("screen")
        .child(
            El::new()
                .id("loading_backdrop")
                .paint("loading_backdrop")
                .attach("LoadingView", view)
                .on_click("loading.continue")
                .focusable(true)
                .with_style(|s| s.fill_parent().pick(Pick::Block)),
        )
        .child(info)
}

/// Builder for `ui.show {"screen": "loading", ...}`.
pub fn build_screen(world: &mut World, args: &Value) -> Result<Entity, String> {
    let map = world.get_resource::<MapData>().cloned().unwrap_or_default();
    let id = args
        .get("planet")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| map.player.planet.clone());
    let (system, planet) = map
        .planet(&id)
        .map(|(s, p)| (s.name.clone(), p.clone()))
        .ok_or_else(|| format!("There is no planet \"{id}\" on the map."))?;
    let place = args.get("place").and_then(Value::as_str).unwrap_or("");
    let tips: Vec<String> = match args.get("tips").and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        None => DEFAULT_TIPS.iter().map(|s| s.to_string()).collect(),
    };
    let progress_path = args
        .get("progress")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let start = args
        .get("at")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0) as f32;
    // `tip` picks the first tip, so two loads in a row needn't open on the same one.
    let first_tip = args.get("tip").and_then(Value::as_u64).unwrap_or(0) as usize;
    let view = LoadingView {
        time: 0.0,
        tip_from: first_tip,
        progress: start,
        shown: start,
        planet: id,
        tips,
        ready_for: 0.0,
    };
    let el = build(world, &planet, &system, place, &view);
    let sp = screens::spawn_screen(
        world,
        Screen {
            modal: true,
            layer: 80,
            closable: false,
            ..Default::default()
        },
        &el,
    )?;
    let root = sp.root.ok_or("empty")?;
    world.entity_mut(root).insert(LoadingScreen {
        progress_path,
        simulate_s: args
            .get("simulate_s")
            .and_then(Value::as_f64)
            .unwrap_or(5.0)
            .max(0.1) as f32,
        start,
        hold: args.get("hold").and_then(Value::as_bool).unwrap_or(true),
    });
    Ok(root)
}

/// Drives progress (from the data path or the simulation), eases the bar, and closes the
/// screen once loading is done and the player continues.
pub fn loading_system(world: &mut World, mut seen: Local<MessageCursor<UiEvent>>) {
    let events: Vec<UiEvent> = crate::input::read_events(world, &mut seen);
    let mut q = world.query::<(Entity, &Screen, &LoadingScreen)>();
    let Some((root, ls)) = q
        .iter(world)
        .find(|(_, s, _)| !s.closing)
        .map(|(e, _, l)| (e, l.clone()))
    else {
        return;
    };
    let Some(bg) = screens::find_in(world, root, "loading_backdrop") else {
        return;
    };
    let dt = world.resource::<Time>().real_delta_secs().min(0.1);
    let from_data = (!ls.progress_path.is_empty())
        .then(|| {
            world
                .get_resource::<UiData>()
                .and_then(|d| d.get_f64(&ls.progress_path))
        })
        .flatten();
    let Some(mut v) = world.get_mut::<LoadingView>(bg) else {
        return;
    };
    v.time += dt;
    let target = match from_data {
        Some(p) => p as f32,
        None if ls.progress_path.is_empty() => {
            // A simulated load: quick at first, a slow middle, a quick finish, like a real one.
            let k = (v.time / ls.simulate_s).min(1.0);
            let eased = k + 0.08 * (k * TAU).sin();
            ls.start + (1.0 - ls.start) * eased.clamp(0.0, 1.0)
        }
        None => v.progress,
    };
    v.progress = v.progress.max(target.clamp(0.0, 1.0));
    let gap = v.progress - v.shown;
    v.shown = if gap < 0.002 {
        v.progress
    } else {
        v.shown + gap * (1.0 - (-dt * 7.0).exp())
    };
    let ready = v.ready();
    if ready {
        v.ready_for += dt;
    }
    let ready_for = v.ready_for;
    let wants_continue = events.iter().any(|ev| {
        matches!(ev, UiEvent::Action { action, entity, .. }
            if action == "loading.continue"
                && entity.is_none_or(|e| screens::is_descendant(world, e, root) || e == root))
    });
    // A short beat after the bar fills before the screen can be dismissed, so a key held from
    // the menu doesn't skip it.
    if ready && ready_for > 0.3 && (wants_continue || !ls.hold) {
        screens::close_screen(world, root, false);
        crate::input::dispatch(world, "game.loading_done");
    }
}
