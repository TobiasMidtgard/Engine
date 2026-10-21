//! The settings screen's detail panel: what the highlighted row does, its default and range,
//! and a live preview (`paint: "setting_preview"`) drawn from the value the row edits, so a
//! slider's effect shows while it is dragged.

use bevy_ecs::prelude::*;
use engine_core::math::Vec2;
use engine_input::InputMap;
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

use crate::color::Color;
use crate::draw::{DrawCtx, Painters};
use crate::el::El;
use crate::menus::{self, ACCENTS, SettingsScreen};
use crate::node::Rect;
use crate::screens;
use crate::style::{Align, Corners, Fill, Overflow, pct};
use crate::theme::Theme;
use crate::widgets::{Slider, Toggle};

/// Help for one settings row, shown while the row is highlighted.
pub struct Help {
    pub id: &'static str,
    pub title: &'static str,
    pub text: &'static str,
    /// What reads the value (a chip on the panel).
    pub tag: &'static str,
    /// The preview painter's kind.
    pub preview: &'static str,
    pub default: &'static str,
}

const fn h(
    id: &'static str,
    title: &'static str,
    tag: &'static str,
    preview: &'static str,
    default: &'static str,
    text: &'static str,
) -> Help {
    Help {
        id,
        title,
        text,
        tag,
        preview,
        default,
    }
}

/// Every settings row with help, by row id.
pub const HELP: [Help; 23] = [
    h(
        "set_fov",
        "Field of view",
        "Camera",
        "fov",
        "80°",
        "How much of the world fits on screen. Wider shows more around you but makes distant things smaller; narrower feels closer and steadier.",
    ),
    h(
        "set_sens",
        "Look sensitivity",
        "Camera",
        "sens",
        "1.00×",
        "How far the view turns for the same swipe of the mouse or push of the stick. Lower is steadier for aiming, higher turns around faster.",
    ),
    h(
        "set_invert",
        "Invert look",
        "Camera",
        "invert",
        "Off",
        "Pushing up looks down, like a flight stick. Leave it off if moving the mouse up should look up.",
    ),
    h(
        "set_shake",
        "Camera shake",
        "Camera",
        "shake",
        "On",
        "Explosions, landings and heavy hits jolt the view. Turn it off if motion makes you uncomfortable.",
    ),
    h(
        "set_aim",
        "Aim assist",
        "Gameplay",
        "aim",
        "On",
        "With a controller, the crosshair slows over targets and leans toward them a little. It never aims for you.",
    ),
    h(
        "set_subs",
        "Subtitles",
        "Gameplay",
        "subs",
        "On",
        "Spoken lines appear at the bottom of the screen with the speaker's name, in conversations and in the world.",
    ),
    h(
        "set_bloom",
        "Bloom",
        "Renderer",
        "bloom",
        "0.04",
        "The soft glow that spills around the sun, lamps, fire and magic. Higher looks dreamier, lower keeps bright edges crisp.",
    ),
    h(
        "set_sat",
        "Saturation",
        "Renderer",
        "sat",
        "1.05",
        "How strong colors are. 1.00 is the image as lit; below that fades toward grey, above makes colors richer.",
    ),
    h(
        "set_contrast",
        "Contrast",
        "Renderer",
        "contrast",
        "1.00",
        "The spread between shadows and highlights. Higher gives deeper shadows and punchier highlights; lower lifts detail out of the dark.",
    ),
    h(
        "set_vignette",
        "Vignette",
        "Renderer",
        "vignette",
        "0.20",
        "Darkens the corners of the screen like a camera lens, drawing the eye to the middle.",
    ),
    h(
        "set_grain",
        "Film grain",
        "Renderer",
        "grain",
        "0.00",
        "A fine moving noise over the image, like film. A little hides color banding in skies and fog.",
    ),
    h(
        "set_shadow",
        "Shadow distance",
        "Renderer",
        "shadow",
        "350 m",
        "How far away things still cast sharp shadows. Further looks better over open land but costs frame rate.",
    ),
    h(
        "set_vsync",
        "Vertical sync",
        "Renderer",
        "vsync",
        "On",
        "Waits for the screen before showing each frame, so the image never tears. Can add a little input delay.",
    ),
    h(
        "set_master",
        "Master volume",
        "Mixer",
        "volume",
        "0.90",
        "The loudness of everything the game plays. The sliders below set each kind of sound against this.",
    ),
    h(
        "set_music",
        "Music",
        "Mixer",
        "volume",
        "0.70",
        "The score and the songs in taverns and markets.",
    ),
    h(
        "set_fx",
        "Effects",
        "Mixer",
        "volume",
        "0.85",
        "Footsteps, weapons, weather, engines, creatures and the interface.",
    ),
    h(
        "set_voice",
        "Voice",
        "Mixer",
        "volume",
        "1.00",
        "Spoken lines in conversations, barks from people nearby and radio calls.",
    ),
    h(
        "set_mute",
        "Mute in background",
        "Mixer",
        "mute",
        "On",
        "Goes quiet while another window is in front, and comes back when you return to the game.",
    ),
    h(
        "set_scale",
        "Interface scale",
        "Interface",
        "scale",
        "1.00×",
        "The size of menus, text and the HUD. Larger reads better from a couch or on a small screen.",
    ),
    h(
        "set_tooltip",
        "Tooltip delay",
        "Interface",
        "tooltip",
        "0.35 s",
        "How long the pointer rests on something before its tooltip opens.",
    ),
    h(
        "set_minimap",
        "Show minimap",
        "HUD",
        "minimap",
        "On",
        "The small round map in the corner of the screen, with nearby markers and the way you face.",
    ),
    h(
        "set_dmg",
        "Damage numbers",
        "HUD",
        "dmg",
        "On",
        "Numbers rise from whatever you hit, gold for critical hits.",
    ),
    h(
        "set_motion",
        "Reduce motion",
        "Interface",
        "motion",
        "Off",
        "Menus fade instead of sliding and bouncing, and pulsing highlights hold still.",
    ),
];

/// The help for a row id (key bindings and accent swatches have their own).
pub fn help(id: &str) -> Option<&'static Help> {
    HELP.iter().find(|h| h.id == id)
}

/// Whether the detail panel can describe the row with this id.
pub fn known(id: &str) -> bool {
    help(id).is_some() || id.starts_with("bind:") || id.starts_with("accent:")
}

/// The first row a tab's detail panel shows before anything is highlighted.
pub fn first_of_tab(world: &World, tab: usize) -> Option<String> {
    match tab {
        0 => Some("set_fov".into()),
        1 => Some("set_bloom".into()),
        2 => Some("set_master".into()),
        3 => world
            .get_resource::<InputMap>()
            .and_then(|m| m.actions.keys().next().map(|a| format!("bind:{a}"))),
        _ => Some("set_scale".into()),
    }
}

/// What a preview draws and the value it reads (on the node with `paint: "setting_preview"`).
#[derive(Component, Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SettingPreview {
    pub kind: String,
    /// The binding path the row edits (empty = none).
    pub path: String,
    pub min: f32,
    pub max: f32,
    /// Decimals of the value badge (-1 = an on/off switch, -2 = no badge).
    pub decimals: i32,
    pub suffix: String,
    /// Accent swatch index, or nothing.
    pub index: usize,
}

/// The preview for the row `id`, reading the slider or toggle under `row` for its binding.
pub fn preview_for(world: &World, id: &str, row: Option<Entity>) -> SettingPreview {
    if let Some(a) = id.strip_prefix("bind:") {
        return SettingPreview {
            kind: "bind".into(),
            path: a.into(),
            decimals: -2,
            ..Default::default()
        };
    }
    if let Some(i) = id.strip_prefix("accent:").and_then(|i| i.parse().ok()) {
        return SettingPreview {
            kind: "accent".into(),
            index: i,
            decimals: -2,
            ..Default::default()
        };
    }
    let mut p = SettingPreview {
        kind: help(id).map_or("icon", |h| h.preview).into(),
        decimals: -2,
        max: 1.0,
        ..Default::default()
    };
    let Some(row) = row else {
        return p;
    };
    if let Some(t) = world.get::<Toggle>(row) {
        p.path = t.bind.clone();
        p.decimals = -1;
    } else if let Some(s) = screens::subtree(world, row)
        .into_iter()
        .find_map(|e| world.get::<Slider>(e))
    {
        p.path = s.bind.clone();
        p.min = s.min;
        p.max = s.max;
        p.decimals = s.decimals.max(0);
        p.suffix = s.suffix.clone();
    }
    p
}

fn fmt(v: f32, decimals: i32, suffix: &str) -> String {
    format!("{v:.prec$}{suffix}", prec = decimals.max(0) as usize)
}

fn fact(label: &str, value: String) -> El {
    El::new()
        .class("row")
        .with_style(|s| s.align_items(Align::Center).gap(10.0))
        .child(El::text(label.to_uppercase()).class("label_caps"))
        .child(El::new().with_style(|s| s.grow(1.0)))
        .child(
            El::text(value)
                .text_style(|t| t.nowrap().tabular())
                .with_style(|s| s.font_size(14.0).font_weight(600)),
        )
}

/// The detail panel's contents for row `id`.
pub fn detail_children(world: &World, id: &str, preview: &SettingPreview) -> Vec<El> {
    let line = world.resource::<Theme>().palette.line;
    let mut out = vec![
        El::new()
            .id("settings_preview")
            .paint("setting_preview")
            .attach("SettingPreview", preview)
            .pick_ignore()
            .with_style(|s| {
                s.width(pct(100.0))
                    .height(214.0)
                    .shrink(0.0)
                    .radius(Corners::all(10.0))
                    .border(1.0, line)
                    .overflow(Overflow::Hidden)
            }),
    ];
    let (title, tag, text, facts) = if let Some(a) = id.strip_prefix("bind:") {
        let keys = world
            .get_resource::<InputMap>()
            .and_then(|m| m.actions.get(a))
            .map(|b| {
                b.buttons
                    .iter()
                    .filter_map(menus::binding_label)
                    .map(|(l, _)| l)
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .unwrap_or_default();
        (
            menus::pretty_action(a),
            "Controls".to_string(),
            "Click the row, then press the key you want. Esc keeps the old one. Controller buttons stay as they are.".to_string(),
            vec![fact("Bound to", if keys.is_empty() { "Nothing".into() } else { keys })],
        )
    } else if let Some((name, ..)) = id
        .strip_prefix("accent:")
        .and_then(|i| i.parse::<usize>().ok())
        .and_then(|i| ACCENTS.get(i))
    {
        (
            format!("{name} accent"),
            "Theme".to_string(),
            "Colors buttons, highlights, bars and the selected tab across every screen. Click to use it.".to_string(),
            vec![fact("Default", "Gold".into())],
        )
    } else if let Some(h) = help(id) {
        let mut facts = vec![fact("Default", h.default.into())];
        if preview.decimals >= 0 {
            facts.push(fact(
                "Range",
                format!(
                    "{} – {}",
                    fmt(preview.min, preview.decimals, &preview.suffix),
                    fmt(preview.max, preview.decimals, &preview.suffix)
                ),
            ));
        }
        (h.title.into(), h.tag.into(), h.text.into(), facts)
    } else {
        return out;
    };
    out.push(
        El::new()
            .class("row")
            .with_style(|s| s.align_items(Align::Center).gap(10.0))
            .child(
                El::text(title)
                    .class("heading")
                    .id("settings_detail_title")
                    .with_style(|s| s.grow(1.0)),
            )
            .child(El::text(tag).class("chip")),
    );
    out.push(
        El::text(text)
            .class("body")
            .with_style(|s| s.font_size(14.5)),
    );
    out.push(El::new().class("divider"));
    out.extend(facts);
    out
}

/// The detail panel itself (id `settings_detail`).
pub fn detail_el(world: &World, id: Option<&str>, preview: &SettingPreview) -> El {
    let p = &world.resource::<Theme>().palette;
    El::new()
        .id("settings_detail")
        .with_style(|s| {
            s.column()
                .width(400.0)
                .shrink(0.0)
                .gap(14.0)
                .padding(16.0)
                .radius(Corners::all(12.0))
                .background(p.well)
                .border(1.0, p.line)
        })
        .children(
            id.map(|id| detail_children(world, id, preview))
                .unwrap_or_default(),
        )
}

// ---------------------------------------------------------------------------- preview

/// Register the preview painter.
pub fn add_painters(p: &mut Painters) {
    p.add("setting_preview", paint_preview);
}

fn hash(i: u32, seed: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9e37_79b9) ^ seed.wrapping_mul(0x85eb_ca6b);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2c1b_3c6d);
    x ^= x >> 12;
    (x & 0xffff) as f32 / 65535.0
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn circle(ctx: &mut DrawCtx, c: Vec2, r: f32, color: Color) {
    ctx.rect(
        Rect::new(c.x - r, c.y - r, r * 2.0, r * 2.0),
        color,
        r,
        0.0,
        Color::TRANSPARENT,
    );
}

/// A dusk valley: sky, sun, two ranges of hills and ground, each color passed through
/// `grade`. Returns the horizon's y.
fn landscape(ctx: &mut DrawCtx, r: Rect, sun_glow: f32, grade: &dyn Fn(Color) -> Color) -> f32 {
    let hy = r.y + r.h * 0.64;
    ctx.rect_fill(
        Rect::new(r.x, r.y, r.w, hy - r.y + 1.0),
        Fill::vertical(grade(Color::hex(0x1a2a4c)), grade(Color::hex(0xd58c5c))),
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    let sun = Vec2::new(r.x + r.w * 0.68, hy - r.h * 0.16);
    let warm = grade(Color::hex(0xffc27a));
    ctx.glow(
        sun,
        r.h * (0.16 + 0.9 * sun_glow),
        warm.with_alpha(0.12 + 0.5 * sun_glow),
        1.3,
    );
    ctx.glow(sun, r.h * 0.12, warm.with_alpha(0.35), 1.8);
    circle(ctx, sun, r.h * 0.05, grade(Color::hex(0xfff1d6)));
    // Rolling ranges: the tops of big circles, far ones paler in the haze.
    let haze = grade(Color::hex(0x8a7a8a));
    for (x, rad, cap, c, mist) in [
        (0.08, 0.30, 0.18, 0x4a4a68, 0.45),
        (0.42, 0.42, 0.13, 0x4a4a68, 0.45),
        (0.86, 0.34, 0.21, 0x4a4a68, 0.45),
        (0.24, 0.36, 0.09, 0x2e3248, 0.15),
        (0.66, 0.40, 0.11, 0x2e3248, 0.15),
        (1.02, 0.30, 0.08, 0x2e3248, 0.15),
    ] {
        let rad = r.w * rad;
        let center = Vec2::new(r.x + r.w * x, hy - r.h * cap + rad);
        circle(ctx, center, rad, grade(Color::hex(c)).lerp(haze, mist));
    }
    ctx.rect_fill(
        Rect::new(r.x, hy, r.w, r.y + r.h - hy),
        Fill::vertical(grade(Color::hex(0x3a4128)), grade(Color::hex(0x141a10))),
        [0.0; 4],
        0.0,
        Color::TRANSPARENT,
    );
    // A lantern-lit cottage and a few trees.
    let px = ctx.px;
    for i in 0..9 {
        let tx = r.x + r.w * (0.03 + 0.11 * i as f32 + 0.05 * hash(i, 3));
        let th = (16.0 + 16.0 * hash(i, 5)) * px;
        let ty = hy + (5.0 + 12.0 * hash(i, 9)) * px;
        let crown = th * 0.36;
        ctx.rect(
            Rect::new(tx - 1.2 * px, ty - th * 0.5, 2.4 * px, th * 0.5),
            grade(Color::hex(0x1a140e)),
            0.0,
            0.0,
            Color::TRANSPARENT,
        );
        let leaf = grade(Color::hex(0x1e2a18));
        circle(ctx, Vec2::new(tx, ty - th * 0.62), crown, leaf);
        circle(
            ctx,
            Vec2::new(tx - crown * 0.5, ty - th * 0.45),
            crown * 0.75,
            leaf,
        );
        circle(
            ctx,
            Vec2::new(tx + crown * 0.5, ty - th * 0.47),
            crown * 0.7,
            leaf.scale(0.85),
        );
    }
    let cot = Rect::new(r.x + r.w * 0.36, hy + 4.0 * px, 34.0 * px, 20.0 * px);
    ctx.rect(
        cot,
        grade(Color::hex(0x2a2018)),
        2.0 * px,
        0.0,
        Color::TRANSPARENT,
    );
    let lamp = Vec2::new(cot.x + cot.w * 0.7, cot.y + cot.h * 0.45);
    ctx.glow(
        lamp,
        (14.0 + 40.0 * sun_glow) * px,
        warm.with_alpha(0.25 + 0.45 * sun_glow),
        1.4,
    );
    circle(ctx, lamp, 2.5 * px, grade(Color::hex(0xffe0a0)));
    hy
}

fn badge(ctx: &mut DrawCtx, r: Rect, text: &str, color: Color, fill: Color) {
    let px = ctx.px;
    let pos = Vec2::new(r.x + r.w - 12.0 * px, r.y + 12.0 * px);
    let probe = ctx.text(
        text,
        15.0,
        700,
        Color::TRANSPARENT,
        pos,
        Vec2::new(1.0, 0.0),
    );
    let pill = probe.inflate(6.0 * px);
    ctx.rect(
        pill,
        fill,
        pill.h * 0.5,
        1.0 * px,
        Color::WHITE.with_alpha(0.12),
    );
    ctx.text(text, 15.0, 700, color, pos, Vec2::new(1.0, 0.0));
}

fn caption(ctx: &mut DrawCtx, r: Rect, text: &str) {
    let px = ctx.px;
    ctx.text(
        text,
        12.0,
        700,
        Color::WHITE.with_alpha(0.72),
        Vec2::new(r.x + 12.0 * px, r.y + r.h - 10.0 * px),
        Vec2::new(0.0, 1.0),
    );
}

fn paint_preview(world: &mut World, e: Entity, ctx: &mut DrawCtx) {
    let Some(p) = world.get::<SettingPreview>(e).cloned() else {
        return;
    };
    let pal = world.resource::<Theme>().palette.clone();
    let v = if p.path.is_empty() || p.kind == "bind" {
        p.min
    } else {
        crate::data::lookup_f64(world, &p.path).unwrap_or(p.min as f64) as f32
    };
    let k = if p.max > p.min {
        ((v - p.min) / (p.max - p.min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let on = v > 0.5;
    let r = ctx.rect;
    let px = ctx.px;
    let t = ctx.time;
    let accent = pal.accent;
    let id = |c: Color| c;
    ctx.rect_fill(
        r,
        Fill::vertical(Color::hex(0x101829), Color::hex(0x080c16)),
        [10.0 * px; 4],
        0.0,
        Color::TRANSPARENT,
    );
    let c = r.center();
    ctx.with_clip(r, |ctx| match p.kind.as_str() {
        "fov" => {
            ctx.rect_fill(
                r,
                Fill::vertical(Color::hex(0x172419), Color::hex(0x0c140e)),
                [0.0; 4],
                0.0,
                Color::TRANSPARENT,
            );
            let cam = Vec2::new(c.x, r.y + r.h - 18.0 * px);
            let reach = r.h * 1.1;
            let half = (v * 0.5).to_radians();
            let dir = |a: f32| Vec2::new(a.sin(), -a.cos());
            for lim in [p.min, p.max] {
                let a = (lim * 0.5).to_radians();
                for s in [-1.0, 1.0] {
                    ctx.line(
                        cam,
                        cam + dir(a * s) * reach,
                        1.0 * px,
                        Color::WHITE.with_alpha(0.12),
                        6.0 * px,
                    );
                }
            }
            ctx.ring(
                cam,
                Vec2::splat(reach * 0.5),
                reach,
                accent.with_alpha(0.13),
                -half,
                half * 2.0,
                0.0,
            );
            for i in 0..34 {
                let tp = Vec2::new(
                    r.x + r.w * (0.04 + 0.92 * hash(i, 11)),
                    r.y + r.h * (0.06 + 0.7 * hash(i, 17)),
                );
                let d = tp - cam;
                let inside = d.x.atan2(-d.y).abs() <= half;
                let rad = (4.0 + 4.0 * hash(i, 23)) * px;
                if inside {
                    ctx.glow(tp, rad * 2.4, Color::hex(0x8fe39a).with_alpha(0.18), 1.5);
                }
                circle(
                    ctx,
                    tp,
                    rad,
                    if inside {
                        Color::hex(0x7fcf86)
                    } else {
                        Color::hex(0x2c3d30)
                    },
                );
            }
            for s in [-1.0, 1.0] {
                ctx.line(
                    cam,
                    cam + dir(half * s) * reach,
                    2.0 * px,
                    accent.with_alpha(0.85),
                    0.0,
                );
            }
            ctx.glow(cam, 18.0 * px, accent.with_alpha(0.45), 1.5);
            ctx.rect(
                Rect::new(cam.x - 7.0 * px, cam.y - 5.0 * px, 14.0 * px, 10.0 * px),
                Color::hex(0xf7f4ee),
                2.5 * px,
                0.0,
                Color::TRANSPARENT,
            );
            caption(ctx, r, "SEEN FROM ABOVE");
        }
        "sens" => {
            let cc = c + Vec2::new(0.0, -6.0 * px);
            let rad = r.h * 0.3;
            ctx.ring(
                cc,
                Vec2::splat(rad),
                6.0 * px,
                Color::WHITE.with_alpha(0.08),
                0.0,
                TAU,
                0.0,
            );
            for i in 0..12 {
                let a = i as f32 / 12.0 * TAU;
                let d = Vec2::new(a.sin(), -a.cos());
                ctx.line(
                    cc + d * (rad + 8.0 * px),
                    cc + d * (rad + 13.0 * px),
                    1.5 * px,
                    Color::WHITE.with_alpha(0.2),
                    0.0,
                );
            }
            let sweep = (v * 90.0).to_radians().min(TAU);
            let g = smooth((t % 2.2) / 1.2);
            let a = sweep * g;
            ctx.ring(cc, Vec2::splat(rad), 6.0 * px, accent, 0.0, a, 0.0);
            let head = cc + Vec2::new(a.sin(), -a.cos()) * rad;
            ctx.glow(head, 16.0 * px, accent.with_alpha(0.6), 1.5);
            circle(ctx, head, 6.0 * px, Color::hex(0xf7f4ee));
            for d in [
                Vec2::new(1.0, 0.0),
                Vec2::new(-1.0, 0.0),
                Vec2::new(0.0, 1.0),
                Vec2::new(0.0, -1.0),
            ] {
                ctx.line(
                    cc + d * 5.0 * px,
                    cc + d * 13.0 * px,
                    2.0 * px,
                    Color::WHITE.with_alpha(0.8),
                    0.0,
                );
            }
            caption(ctx, r, &format!("ONE SWIPE TURNS {:.0}°", v * 90.0));
        }
        "invert" => {
            let m = (t * 1.8).sin();
            let pitch = if on { -m } else { m };
            // The mouse.
            let mc = Vec2::new(r.x + r.w * 0.28, c.y + m * 14.0 * px);
            ctx.rect(
                Rect::new(mc.x - 17.0 * px, mc.y - 26.0 * px, 34.0 * px, 52.0 * px),
                Color::hex(0x2a3348),
                17.0 * px,
                1.0 * px,
                Color::WHITE.with_alpha(0.25),
            );
            ctx.line(
                mc + Vec2::new(0.0, -26.0 * px),
                mc + Vec2::new(0.0, -8.0 * px),
                1.0 * px,
                Color::WHITE.with_alpha(0.3),
                0.0,
            );
            let arrow = |up: bool| if up { "ui_arrow_up" } else { "ui_arrow_down" };
            ctx.icon(
                arrow(m > 0.0),
                Rect::new(mc.x - 9.0 * px, r.y + 14.0 * px, 18.0 * px, 18.0 * px),
                Color::WHITE.with_alpha(0.3 + 0.6 * m.abs()),
            );
            // The view: looking up drops the horizon.
            let vr = Rect::new(
                r.x + r.w * 0.5,
                r.y + 30.0 * px,
                r.w * 0.42,
                r.h - 62.0 * px,
            );
            ctx.with_clip(vr, |ctx| {
                let hy = vr.y + vr.h * 0.5 + pitch * vr.h * 0.3;
                ctx.rect_fill(
                    vr,
                    Fill::vertical(Color::hex(0x2a4a7a), Color::hex(0xc9905c)),
                    [0.0; 4],
                    0.0,
                    Color::TRANSPARENT,
                );
                ctx.rect(
                    Rect::new(vr.x, hy, vr.w, vr.h),
                    Color::hex(0x2c3a22),
                    0.0,
                    0.0,
                    Color::TRANSPARENT,
                );
            });
            ctx.rect(
                vr,
                Color::TRANSPARENT,
                6.0 * px,
                1.5 * px,
                Color::WHITE.with_alpha(0.35),
            );
            ctx.icon(
                arrow(pitch > 0.0),
                Rect::new(
                    vr.x + vr.w * 0.5 - 9.0 * px,
                    r.y + 8.0 * px,
                    18.0 * px,
                    18.0 * px,
                ),
                accent.with_alpha(0.3 + 0.7 * pitch.abs()),
            );
            caption(
                ctx,
                r,
                if on {
                    "MOUSE UP, LOOK DOWN"
                } else {
                    "MOUSE UP, LOOK UP"
                },
            );
        }
        "shake" => {
            let phase = t % 2.2;
            let e = if on { (-phase * 5.0).exp() } else { 0.0 };
            let off = Vec2::new(
                (t * 53.0).sin() * e * 8.0 * px,
                (t * 41.0).cos() * e * 6.0 * px,
            );
            let sr = Rect::new(
                r.x - 12.0 * px + off.x,
                r.y - 12.0 * px + off.y,
                r.w + 24.0 * px,
                r.h + 24.0 * px,
            );
            let hy = landscape(ctx, sr, 0.2, &id);
            let blast = Vec2::new(sr.x + sr.w * 0.3, hy + 14.0 * px);
            let f = (-phase * 3.5).exp();
            ctx.glow(
                blast,
                60.0 * px * f + 8.0 * px,
                Color::hex(0xffa040).with_alpha(0.8 * f),
                1.2,
            );
            caption(
                ctx,
                r,
                if on {
                    "A BLAST SHAKES THE VIEW"
                } else {
                    "THE VIEW STAYS STILL"
                },
            );
        }
        "aim" => {
            for i in 0..9 {
                let x = r.x + r.w * i as f32 / 8.0;
                ctx.line(
                    Vec2::new(x, r.y),
                    Vec2::new(x, r.y + r.h),
                    1.0 * px,
                    Color::WHITE.with_alpha(0.04),
                    0.0,
                );
            }
            let tp = c + Vec2::new((t * 0.9).sin() * r.w * 0.26, (t * 1.7).sin() * r.h * 0.18);
            let aim = c + Vec2::new(
                (t * 0.9 - 0.5).sin() * r.w * 0.3,
                (t * 1.3 + 1.0).cos() * r.h * 0.22,
            );
            let zone = 46.0 * px;
            let d = (tp - aim).length();
            let pull = if on {
                0.75 * (1.0 - d / (zone * 1.6)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let ret = aim + (tp - aim) * pull;
            if on {
                ctx.ring(
                    tp,
                    Vec2::splat(zone),
                    1.5 * px,
                    accent.with_alpha(0.35),
                    0.0,
                    TAU,
                    18.0,
                );
            }
            ctx.glow(tp, 22.0 * px, Color::hex(0xff6a5a).with_alpha(0.35), 1.5);
            circle(ctx, tp, 9.0 * px, Color::hex(0xff6a5a));
            circle(ctx, tp, 4.0 * px, Color::hex(0xffe0d8));
            if pull > 0.02 {
                ctx.ring(
                    aim,
                    Vec2::splat(12.0 * px),
                    1.0 * px,
                    Color::WHITE.with_alpha(0.25),
                    0.0,
                    TAU,
                    10.0,
                );
                ctx.line(aim, ret, 1.5 * px, accent.with_alpha(0.6), 4.0 * px);
            }
            ctx.ring(
                ret,
                Vec2::splat(14.0 * px),
                2.0 * px,
                Color::WHITE,
                0.0,
                TAU,
                0.0,
            );
            circle(ctx, ret, 2.0 * px, Color::WHITE);
            caption(
                ctx,
                r,
                if on {
                    "THE CROSSHAIR LEANS IN"
                } else {
                    "NO HELP AIMING"
                },
            );
        }
        "subs" => {
            ctx.rect_fill(
                r,
                Fill::Radial {
                    inner: Color::hex(0x2a2530),
                    outer: Color::hex(0x0a0a10),
                },
                [0.0; 4],
                0.0,
                Color::TRANSPARENT,
            );
            for (x, s, col) in [(0.36, 1.0, 0x6a5a50), (0.64, 0.92, 0x4a5a70)] {
                let hc = Vec2::new(r.x + r.w * x, r.y + r.h * 0.4);
                ctx.glow(hc, 50.0 * px, Color::hex(col).with_alpha(0.25), 1.4);
                circle(ctx, hc, 16.0 * px * s, Color::hex(col));
                ctx.rect(
                    Rect::new(
                        hc.x - 26.0 * px * s,
                        hc.y + 20.0 * px,
                        52.0 * px * s,
                        70.0 * px,
                    ),
                    Color::hex(col).scale(0.8),
                    20.0 * px,
                    0.0,
                    Color::TRANSPARENT,
                );
            }
            let bar = 16.0 * px;
            ctx.rect(
                Rect::new(r.x, r.y, r.w, bar),
                Color::BLACK,
                0.0,
                0.0,
                Color::TRANSPARENT,
            );
            ctx.rect(
                Rect::new(r.x, r.y + r.h - bar, r.w, bar),
                Color::BLACK,
                0.0,
                0.0,
                Color::TRANSPARENT,
            );
            let by = r.y + r.h - bar - 12.0 * px;
            if on {
                let line = "The pass closes at dusk. Hurry.";
                let probe = ctx.text(
                    line,
                    14.0,
                    600,
                    Color::TRANSPARENT,
                    Vec2::new(c.x, by),
                    Vec2::new(0.5, 1.0),
                );
                let back = Rect::new(
                    probe.x - 10.0 * px,
                    probe.y - 22.0 * px,
                    probe.w + 20.0 * px,
                    probe.h + 28.0 * px,
                );
                ctx.rect(
                    back,
                    Color::BLACK.with_alpha(0.55),
                    6.0 * px,
                    0.0,
                    Color::TRANSPARENT,
                );
                ctx.text(
                    "MARA",
                    11.0,
                    800,
                    pal.accent_light,
                    Vec2::new(c.x, probe.y - 4.0 * px),
                    Vec2::new(0.5, 1.0),
                );
                ctx.text(
                    line,
                    14.0,
                    600,
                    Color::WHITE,
                    Vec2::new(c.x, by),
                    Vec2::new(0.5, 1.0),
                );
            } else {
                for i in 0..9 {
                    let hh = (4.0 + 12.0 * (0.5 + 0.5 * (t * 7.0 + i as f32 * 1.3).sin())) * px;
                    let x = c.x + (i as f32 - 4.0) * 7.0 * px;
                    ctx.rect(
                        Rect::new(x - 2.0 * px, by - 10.0 * px - hh * 0.5, 4.0 * px, hh),
                        Color::WHITE.with_alpha(0.4),
                        2.0 * px,
                        0.0,
                        Color::TRANSPARENT,
                    );
                }
            }
        }
        "bloom" => {
            landscape(ctx, r, k, &id);
        }
        "sat" => {
            let grade = |c: Color| {
                let g = 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;
                let f = |x: f32| (g + (x - g) * v).clamp(0.0, 1.0);
                Color::linear(f(c.r), f(c.g), f(c.b), c.a)
            };
            landscape(ctx, r, 0.25, &grade);
            let colors = [0xd84a3a, 0xe8963a, 0xe8d04a, 0x5ab85a, 0x3a8ad8, 0x8a5ad8];
            let sw = 22.0 * px;
            let x0 = r.x + 12.0 * px;
            let y0 = r.y + r.h - sw - 12.0 * px;
            for (i, col) in colors.iter().enumerate() {
                ctx.rect(
                    Rect::new(x0 + i as f32 * (sw + 5.0 * px), y0, sw, sw),
                    grade(Color::hex(*col)),
                    5.0 * px,
                    1.0 * px,
                    Color::WHITE.with_alpha(0.2),
                );
            }
        }
        "contrast" => {
            let grade = |c: Color| {
                let f = |x: f32| (0.2 * (x.max(0.0) / 0.2).powf(v)).clamp(0.0, 1.0);
                Color::linear(f(c.r), f(c.g), f(c.b), c.a)
            };
            landscape(ctx, r, 0.25, &grade);
            let n = 9;
            let sw = 20.0 * px;
            let x0 = r.x + 12.0 * px;
            let y0 = r.y + r.h - sw - 12.0 * px;
            for i in 0..n {
                let l = 0.008 * (0.9f32 / 0.008).powf(i as f32 / (n - 1) as f32);
                ctx.rect(
                    Rect::new(x0 + i as f32 * sw, y0, sw, sw),
                    grade(Color::linear(l, l, l, 1.0)),
                    0.0,
                    0.0,
                    Color::TRANSPARENT,
                );
            }
            ctx.rect(
                Rect::new(x0, y0, sw * n as f32, sw),
                Color::TRANSPARENT,
                4.0 * px,
                1.0 * px,
                Color::WHITE.with_alpha(0.25),
            );
        }
        "vignette" => {
            landscape(ctx, r, 0.25, &id);
            ctx.rect_fill(
                r,
                Fill::Radial {
                    inner: Color::BLACK.with_alpha(0.0),
                    outer: Color::BLACK.with_alpha((v * 1.1).min(1.0)),
                },
                [0.0; 4],
                0.0,
                Color::TRANSPARENT,
            );
        }
        "grain" => {
            landscape(ctx, r, 0.25, &id);
            let frame = (t * 24.0) as u32;
            let a = k.sqrt() * 0.6;
            if a > 0.01 {
                let s = 2.0 * px;
                for i in 0..700 {
                    let x = r.x + r.w * hash(i, frame);
                    let y = r.y + r.h * hash(i, frame ^ 0x55aa);
                    let col = if hash(i, frame + 7) > 0.5 {
                        Color::WHITE
                    } else {
                        Color::BLACK
                    };
                    ctx.rect(
                        Rect::new(x, y, s, s),
                        col.with_alpha(a * (0.4 + 0.6 * hash(i, 99))),
                        0.0,
                        0.0,
                        Color::TRANSPARENT,
                    );
                }
            }
        }
        "shadow" => {
            let hy = r.y + r.h * 0.3;
            ctx.rect_fill(
                Rect::new(r.x, r.y, r.w, hy - r.y),
                Fill::vertical(Color::hex(0x3a5a8a), Color::hex(0xd8b890)),
                [0.0; 4],
                0.0,
                Color::TRANSPARENT,
            );
            ctx.rect_fill(
                Rect::new(r.x, hy, r.w, r.y + r.h - hy),
                Fill::vertical(Color::hex(0x6a6a48), Color::hex(0x8a8458)),
                [0.0; 4],
                0.0,
                Color::TRANSPARENT,
            );
            let vp = Vec2::new(r.x + r.w * 0.36, hy);
            for s in [-1.0, 1.0] {
                ctx.line(
                    vp,
                    Vec2::new(vp.x + s * r.w * 0.5, r.y + r.h),
                    1.5 * px,
                    Color::WHITE.with_alpha(0.25),
                    0.0,
                );
            }
            let f = |d: f32| 1.0 / (1.0 + d / 18.0);
            let f10 = f(10.0);
            let span = r.y + r.h - hy;
            for d in [10.0, 25.0, 50.0, 100.0, 200.0, 350.0, 600.0] {
                let q = f(d) / f10;
                let base = Vec2::new(vp.x + r.w * 0.5 * q, hy + span * 0.92 * q);
                let ph = 70.0 * px * q;
                let pw = (10.0 * px * q).max(2.0 * px);
                if d <= v {
                    ctx.line(
                        base,
                        base + Vec2::new(-ph * 0.9, ph * 0.12),
                        pw * 0.9,
                        Color::BLACK.with_alpha(0.5),
                        0.0,
                    );
                }
                ctx.rect(
                    Rect::new(base.x - pw * 0.5, base.y - ph, pw, ph),
                    Color::hex(0xe8d8b8),
                    pw * 0.3,
                    0.0,
                    Color::TRANSPARENT,
                );
                if d >= 25.0 {
                    ctx.text(
                        &format!("{d:.0}"),
                        10.0,
                        700,
                        if d <= v {
                            Color::hex(0x1a1a10)
                        } else {
                            Color::hex(0x1a1a10).with_alpha(0.45)
                        },
                        Vec2::new(base.x, base.y + 3.0 * px),
                        Vec2::new(0.5, 0.0),
                    );
                }
            }
        }
        "vsync" => {
            let x = r.x + r.w * ((t * 0.55) % 1.0) * 1.3 - r.w * 0.15;
            let bands = 3;
            for b in 0..bands {
                let y0 = r.y + r.h * b as f32 / bands as f32;
                let hh = r.h / bands as f32;
                let shift = if on { 0.0 } else { [0.0, 22.0, -12.0][b] * px };
                ctx.with_clip(Rect::new(r.x, y0, r.w, hh), |ctx| {
                    ctx.rect_fill(
                        Rect::new(
                            x + shift - 30.0 * px,
                            r.y + 14.0 * px,
                            60.0 * px,
                            r.h - 28.0 * px,
                        ),
                        Fill::Linear {
                            from: pal.accent_light,
                            to: pal.accent_dark,
                            angle_deg: 90.0,
                        },
                        [8.0 * px; 4],
                        0.0,
                        Color::TRANSPARENT,
                    );
                });
                if !on && b > 0 {
                    ctx.line(
                        Vec2::new(r.x, y0),
                        Vec2::new(r.x + r.w, y0),
                        1.0 * px,
                        Color::hex(0xff6a5a).with_alpha(0.6),
                        5.0 * px,
                    );
                }
            }
            caption(
                ctx,
                r,
                if on {
                    "EVERY FRAME WHOLE"
                } else {
                    "TORN FRAMES"
                },
            );
        }
        "volume" | "mute" => {
            let (amp, label) = if p.kind == "mute" {
                let away = (t % 4.0) > 2.0;
                let a = if on && away { 0.0 } else { 1.0 };
                (
                    a,
                    if away {
                        "ANOTHER WINDOW IN FRONT"
                    } else {
                        "GAME IN FRONT"
                    },
                )
            } else {
                (v, "")
            };
            let style = p.path.rsplit('.').next().unwrap_or_default();
            let n = 40;
            let bw = (r.w - 32.0 * px) / n as f32;
            let base = c.y;
            for i in 0..n {
                let fi = i as f32;
                let wave = match style {
                    "music" => {
                        0.5 + 0.5 * (fi * 0.35 + t * 2.0).sin() * (fi * 0.11 - t * 0.7).cos()
                    }
                    "effects" => {
                        let beat = (t * 2.4 + fi * 0.04).fract();
                        (1.0 - beat).powi(3) * (0.4 + 0.6 * hash(i, (t * 10.0) as u32))
                    }
                    "voice" => {
                        let syl = (0.5 + 0.5 * (t * 6.0 - fi * 0.3).sin()).powi(2);
                        syl * (0.6 + 0.4 * (fi * 0.9).sin().abs())
                    }
                    _ => {
                        0.3 + 0.7
                            * (0.5 + 0.5 * (fi * 0.5 + t * 3.1).sin() * (fi * 0.23 + t * 1.7).sin())
                    }
                };
                let hh = (wave * amp * r.h * 0.34).max(1.5 * px);
                let x = r.x + 16.0 * px + fi * bw;
                ctx.rect_fill(
                    Rect::new(x + bw * 0.18, base - hh, bw * 0.64, hh),
                    Fill::vertical(pal.accent_light, accent),
                    [bw * 0.3; 4],
                    0.0,
                    Color::TRANSPARENT,
                );
                ctx.rect(
                    Rect::new(x + bw * 0.18, base + 2.0 * px, bw * 0.64, hh * 0.5),
                    accent.with_alpha(0.22),
                    bw * 0.3,
                    0.0,
                    Color::TRANSPARENT,
                );
            }
            if amp <= 0.001 {
                ctx.text(
                    "MUTED",
                    14.0,
                    800,
                    Color::WHITE.with_alpha(0.75),
                    c + Vec2::new(0.0, -22.0 * px),
                    Vec2::new(0.5, 0.5),
                );
            }
            if !label.is_empty() {
                caption(ctx, r, label);
            }
        }
        "scale" => {
            landscape(ctx, r, 0.2, &id);
            let s = v * px;
            let m = 10.0 * px;
            // Health and stamina, bottom left.
            for (i, (fill, col)) in [(0.72, 0xd85a4a), (0.5, 0x5ad89a)].into_iter().enumerate() {
                let y = r.y + r.h - m - (8.0 + i as f32 * 12.0) * s;
                let w = 110.0 * s;
                ctx.rect(
                    Rect::new(r.x + m, y, w, 7.0 * s),
                    Color::BLACK.with_alpha(0.5),
                    3.5 * s,
                    0.0,
                    Color::TRANSPARENT,
                );
                ctx.rect(
                    Rect::new(r.x + m, y, w * fill, 7.0 * s),
                    Color::hex(col),
                    3.5 * s,
                    0.0,
                    Color::TRANSPARENT,
                );
            }
            // Quest lines, top left.
            ctx.text(
                "Reach the pass",
                12.0 * v,
                700,
                Color::WHITE,
                Vec2::new(r.x + m, r.y + m),
                Vec2::ZERO,
            );
            ctx.rect(
                Rect::new(r.x + m, r.y + m + 18.0 * s, 80.0 * s, 4.0 * s),
                Color::WHITE.with_alpha(0.5),
                2.0 * s,
                0.0,
                Color::TRANSPARENT,
            );
            // Minimap, top right.
            let mr = 26.0 * s;
            let mc = Vec2::new(r.x + r.w - m - mr, r.y + m + mr + 34.0 * px);
            circle(ctx, mc, mr, Color::hex(0x0a1018).with_alpha(0.8));
            ctx.ring(
                mc,
                Vec2::splat(mr),
                1.5 * s,
                accent.with_alpha(0.8),
                0.0,
                TAU,
                0.0,
            );
            circle(ctx, mc, 3.0 * s, accent);
            // Hotbar, bottom middle.
            for i in 0..5 {
                let q = 20.0 * s;
                let x = c.x + (i as f32 - 2.5) * (q + 4.0 * s);
                ctx.rect(
                    Rect::new(x, r.y + r.h - m - q, q, q),
                    Color::BLACK.with_alpha(0.45),
                    4.0 * s,
                    1.0 * px,
                    Color::WHITE.with_alpha(0.3),
                );
            }
        }
        "tooltip" => {
            let period = v + 1.8;
            let phase = t % period;
            let card = Rect::new(r.x + 24.0 * px, c.y - 22.0 * px, 120.0 * px, 44.0 * px);
            ctx.rect(
                card,
                pal.surface_hover,
                8.0 * px,
                1.0 * px,
                pal.line.scale(1.6),
            );
            ctx.icon(
                "sword",
                Rect::new(card.x + 8.0 * px, card.y + 8.0 * px, 28.0 * px, 28.0 * px),
                Color::WHITE,
            );
            ctx.text(
                "Iron sword",
                13.0,
                600,
                pal.text,
                Vec2::new(card.x + 44.0 * px, card.y + card.h * 0.5),
                Vec2::new(0.0, 0.5),
            );
            let cur = Vec2::new(card.x + card.w * 0.7, card.y + card.h * 0.62);
            if phase < v {
                ctx.ring(
                    cur,
                    Vec2::splat(11.0 * px),
                    2.5 * px,
                    accent,
                    0.0,
                    TAU * phase / v.max(0.01),
                    0.0,
                );
            }
            ctx.line(
                cur,
                cur + Vec2::new(0.0, 16.0 * px),
                2.0 * px,
                Color::WHITE,
                0.0,
            );
            ctx.line(
                cur,
                cur + Vec2::new(11.0 * px, 11.0 * px),
                2.0 * px,
                Color::WHITE,
                0.0,
            );
            if phase >= v {
                let a = ((phase - v) / 0.15).min(1.0);
                ctx.faded(a, |ctx| {
                    let tip = Rect::new(
                        card.x + card.w + 14.0 * px,
                        card.y - 30.0 * px,
                        r.w - card.w - 64.0 * px,
                        92.0 * px,
                    );
                    ctx.shadow(tip, 8.0 * px, 12.0 * px, Color::BLACK.with_alpha(0.5));
                    ctx.rect(tip, pal.panel_top, 8.0 * px, 1.0 * px, pal.line.scale(1.6));
                    ctx.text(
                        "Iron sword",
                        14.0,
                        700,
                        pal.rarity[1],
                        Vec2::new(tip.x + 12.0 * px, tip.y + 12.0 * px),
                        Vec2::ZERO,
                    );
                    ctx.text(
                        "One-handed · 14 damage",
                        12.0,
                        500,
                        pal.text_dim,
                        Vec2::new(tip.x + 12.0 * px, tip.y + 36.0 * px),
                        Vec2::ZERO,
                    );
                    ctx.text(
                        "Forged in Landfall.",
                        12.0,
                        500,
                        pal.text_faint,
                        Vec2::new(tip.x + 12.0 * px, tip.y + 58.0 * px),
                        Vec2::ZERO,
                    );
                });
            }
            caption(
                ctx,
                r,
                if v < 0.05 {
                    "OPENS AT ONCE"
                } else {
                    "OPENS AFTER A REST"
                },
            );
        }
        "minimap" => {
            landscape(ctx, r, 0.2, &id);
            let mr = 40.0 * px;
            let mc = Vec2::new(r.x + r.w - mr - 14.0 * px, r.y + mr + 14.0 * px);
            if on {
                circle(ctx, mc, mr, Color::hex(0x0a1220).with_alpha(0.85));
                for i in 0..6 {
                    let a = hash(i, 4) * TAU;
                    let d = mr * (0.3 + 0.6 * hash(i, 8));
                    circle(
                        ctx,
                        mc + Vec2::new(a.sin(), -a.cos()) * d,
                        3.0 * px,
                        if i == 0 {
                            pal.warn
                        } else {
                            pal.info.with_alpha(0.8)
                        },
                    );
                }
                ctx.ring(
                    mc,
                    Vec2::splat(mr),
                    2.0 * px,
                    accent.with_alpha(0.85),
                    0.0,
                    TAU,
                    0.0,
                );
                ctx.icon_rotated(
                    "ui_arrow_up",
                    Rect::new(mc.x - 8.0 * px, mc.y - 8.0 * px, 16.0 * px, 16.0 * px),
                    accent,
                    (t * 0.6).sin() * 0.6,
                );
            } else {
                ctx.ring(
                    mc,
                    Vec2::splat(mr),
                    1.5 * px,
                    Color::WHITE.with_alpha(0.35),
                    0.0,
                    TAU,
                    16.0,
                );
            }
            caption(ctx, r, if on { "SHOWN IN THE CORNER" } else { "HIDDEN" });
        }
        "dmg" => {
            let ec = c + Vec2::new(0.0, 10.0 * px);
            let hit = (t % 0.7) / 0.7;
            ctx.glow(
                ec,
                60.0 * px,
                Color::hex(0xff5a4a).with_alpha(0.25 * (1.0 - hit)),
                1.4,
            );
            circle(
                ctx,
                ec + Vec2::new(0.0, -30.0 * px),
                14.0 * px,
                Color::hex(0x3a2a30),
            );
            ctx.rect(
                Rect::new(ec.x - 24.0 * px, ec.y - 14.0 * px, 48.0 * px, 70.0 * px),
                Color::hex(0x3a2a30),
                18.0 * px,
                1.5 * px,
                Color::hex(0xff6a5a).with_alpha(0.5),
            );
            if on {
                for (i, (n, crit)) in [(24, false), (61, true), (18, false)]
                    .into_iter()
                    .enumerate()
                {
                    let life = ((t + i as f32 * 0.7) % 2.1) / 2.1;
                    let pos =
                        ec + Vec2::new((i as f32 - 1.0) * 46.0 * px, -40.0 * px - life * 50.0 * px);
                    let a = 1.0 - smooth((life - 0.6) / 0.4);
                    let (size, col) = if crit {
                        (24.0, pal.accent_light)
                    } else {
                        (17.0, Color::WHITE)
                    };
                    ctx.text(
                        &n.to_string(),
                        size,
                        800,
                        Color::BLACK.with_alpha(0.6 * a),
                        pos + Vec2::new(1.5 * px, 1.5 * px),
                        Vec2::new(0.5, 0.5),
                    );
                    ctx.text(
                        &n.to_string(),
                        size,
                        800,
                        col.with_alpha(a),
                        pos,
                        Vec2::new(0.5, 0.5),
                    );
                }
            }
            caption(
                ctx,
                r,
                if on {
                    "NUMBERS RISE FROM HITS"
                } else {
                    "HITS SHOW NO NUMBERS"
                },
            );
        }
        "motion" => {
            let phase = (t % 2.6) / 2.6;
            let card = Rect::new(c.x - 90.0 * px, c.y - 34.0 * px, 180.0 * px, 68.0 * px);
            let (dx, a) = if on {
                (0.0, smooth(phase / 0.25))
            } else {
                let q = (phase / 0.35).min(1.0);
                let back = 1.0 + 2.7 * (q - 1.0).powi(3) + 1.7 * (q - 1.0).powi(2);
                ((1.0 - back) * 80.0 * px, q.min(1.0))
            };
            if !on {
                for j in 0..3 {
                    let q = (phase * 1.5 + j as f32 / 3.0).fract();
                    ctx.ring(
                        card.center(),
                        Vec2::new(card.w * (0.5 + q * 0.5), card.h * (0.5 + q * 0.8)),
                        2.0 * px,
                        accent.with_alpha(0.4 * (1.0 - q)),
                        0.0,
                        TAU,
                        0.0,
                    );
                }
            }
            ctx.faded(a, |ctx| {
                let cr = Rect::new(card.x + dx, card.y, card.w, card.h);
                ctx.rect(
                    cr,
                    pal.surface_hover,
                    10.0 * px,
                    1.0 * px,
                    accent.with_alpha(0.7),
                );
                ctx.icon(
                    "ui_check",
                    Rect::new(cr.x + 14.0 * px, cr.y + 20.0 * px, 28.0 * px, 28.0 * px),
                    accent,
                );
                ctx.text(
                    "Quest updated",
                    14.0,
                    700,
                    pal.text,
                    Vec2::new(cr.x + 52.0 * px, cr.y + 22.0 * px),
                    Vec2::ZERO,
                );
                ctx.text(
                    "Reach the pass",
                    12.0,
                    500,
                    pal.text_dim,
                    Vec2::new(cr.x + 52.0 * px, cr.y + 40.0 * px),
                    Vec2::ZERO,
                );
            });
            caption(
                ctx,
                r,
                if on {
                    "FADES IN, HOLDS STILL"
                } else {
                    "SLIDES, BOUNCES AND PULSES"
                },
            );
        }
        "accent" => {
            let Some((_, a, l, d)) = ACCENTS.get(p.index) else {
                return;
            };
            let (a, l, d) = (Color::hex(*a), Color::hex(*l), Color::hex(*d));
            let btn = Rect::new(c.x - 90.0 * px, r.y + 30.0 * px, 180.0 * px, 40.0 * px);
            ctx.glow(btn.center(), 110.0 * px, a.with_alpha(0.25), 1.4);
            ctx.rect_fill(
                btn,
                Fill::vertical(l, a),
                [9.0 * px; 4],
                0.0,
                Color::TRANSPARENT,
            );
            ctx.text(
                "Continue",
                15.0,
                700,
                Color::hex(0x23180a),
                btn.center(),
                Vec2::new(0.5, 0.5),
            );
            let bar = Rect::new(c.x - 120.0 * px, btn.y + 64.0 * px, 240.0 * px, 10.0 * px);
            ctx.rect(bar, pal.well, 5.0 * px, 1.0 * px, pal.line);
            ctx.rect_fill(
                Rect::new(bar.x, bar.y, bar.w * 0.64, bar.h),
                Fill::Linear {
                    from: d,
                    to: l,
                    angle_deg: 90.0,
                },
                [5.0 * px; 4],
                0.0,
                Color::TRANSPARENT,
            );
            for (i, label) in ["Gear", "Map", "Quests"].into_iter().enumerate() {
                let tr = Rect::new(
                    c.x - 120.0 * px + i as f32 * 82.0 * px,
                    bar.y + 30.0 * px,
                    74.0 * px,
                    30.0 * px,
                );
                if i == 1 {
                    ctx.rect(
                        tr,
                        a.with_alpha(0.16),
                        7.0 * px,
                        1.0 * px,
                        a.with_alpha(0.9),
                    );
                }
                ctx.text(
                    label,
                    13.0,
                    600,
                    if i == 1 { l } else { pal.text_dim },
                    tr.center(),
                    Vec2::new(0.5, 0.5),
                );
            }
        }
        // Drawn after, since key caps read the input map.
        "bind" => {}
        _ => {
            ctx.icon(
                "ui_settings",
                Rect::new(c.x - 32.0 * px, c.y - 32.0 * px, 64.0 * px, 64.0 * px),
                Color::WHITE.with_alpha(0.3),
            );
        }
    });
    if p.kind == "bind" {
        paint_bind(world, &p.path, ctx);
    }
    match p.decimals {
        -1 => {
            let (text, color, fill) = if on {
                ("ON", Color::hex(0x23180a), accent)
            } else {
                ("OFF", pal.text_dim, Color::BLACK.with_alpha(0.55))
            };
            badge(ctx, r, text, color, fill);
        }
        d if d >= 0 => badge(
            ctx,
            r,
            &fmt(v, d, &p.suffix),
            Color::WHITE,
            Color::BLACK.with_alpha(0.55),
        ),
        _ => {}
    }
}

/// Key caps for an action's bindings, pulsing while the screen waits for a new key.
fn paint_bind(world: &mut World, action: &str, ctx: &mut DrawCtx) {
    let pal = world.resource::<Theme>().palette.clone();
    let labels: Vec<(String, bool)> = world
        .get_resource::<InputMap>()
        .and_then(|m| m.actions.get(action))
        .map(|b| b.buttons.iter().filter_map(menus::binding_label).collect())
        .unwrap_or_default();
    let waiting = world
        .query::<&SettingsScreen>()
        .iter(world)
        .any(|s| s.rebinding.as_deref() == Some(action));
    let r = ctx.rect;
    let px = ctx.px;
    let c = r.center();
    let t = ctx.time;
    let caps: Vec<(String, bool)> = if waiting {
        vec![("?".into(), false)]
    } else if labels.is_empty() {
        vec![("—".into(), false)]
    } else {
        labels.into_iter().take(3).collect()
    };
    let size = 64.0 * px;
    let gap = 18.0 * px;
    let total = caps.len() as f32 * size + (caps.len() as f32 - 1.0) * gap;
    let pulse = if waiting {
        0.5 + 0.5 * (t * 5.0).sin()
    } else {
        0.0
    };
    for (i, (label, pad)) in caps.iter().enumerate() {
        let x = c.x - total * 0.5 + i as f32 * (size + gap);
        let key = Rect::new(x, c.y - size * 0.62, size, size);
        let radius = if *pad { size * 0.5 } else { 10.0 * px };
        ctx.shadow(
            Rect::new(key.x, key.y + 6.0 * px, key.w, key.h),
            radius,
            10.0 * px,
            Color::BLACK.with_alpha(0.6),
        );
        if waiting {
            ctx.glow(
                key.center(),
                size,
                pal.accent.with_alpha(0.25 + 0.3 * pulse),
                1.4,
            );
        }
        ctx.rect_fill(
            Rect::new(key.x, key.y + 5.0 * px, key.w, key.h),
            Fill::Solid(Color::hex(0x0c0f16)),
            [radius; 4],
            0.0,
            Color::TRANSPARENT,
        );
        ctx.rect_fill(
            key,
            Fill::vertical(Color::hex(0x3a4358), Color::hex(0x232a3a)),
            [radius; 4],
            1.0 * px,
            if waiting {
                pal.accent.with_alpha(0.6 + 0.4 * pulse)
            } else {
                Color::WHITE.with_alpha(0.28)
            },
        );
        let fs = if label.chars().count() > 3 {
            15.0
        } else {
            26.0
        };
        ctx.text(label, fs, 700, pal.text, key.center(), Vec2::new(0.5, 0.5));
    }
    ctx.text(
        if waiting {
            "PRESS A KEY"
        } else {
            "KEYBOARD AND CONTROLLER"
        },
        12.0,
        700,
        if waiting {
            pal.accent_light
        } else {
            Color::WHITE.with_alpha(0.6)
        },
        Vec2::new(c.x, r.y + r.h - 14.0 * px),
        Vec2::new(0.5, 1.0),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_help_row_has_a_preview_and_unique_id() {
        let mut ids = std::collections::BTreeSet::new();
        for h in &HELP {
            assert!(ids.insert(h.id), "{} twice", h.id);
            assert!(!h.preview.is_empty() && !h.text.is_empty() && !h.default.is_empty());
            assert!(h.text.ends_with('.'), "{}: {}", h.id, h.text);
        }
        assert!(known("bind:jump") && known("accent:2") && !known("settings_done"));
    }
}
