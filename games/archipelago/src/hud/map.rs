//! The map table: the whole archipelago as an engraved sea chart on parchment, in a brass
//! frame at the bottom right. The chart itself is painted once from the islands' height
//! function (coasts inked, soundings as thin lines hugging the shore, hills shaded from the
//! north-west, woods stippled); towns, roads, ships, airships, island names and the part of
//! the world in view are drawn over it every frame. A click sends the view there.
//!
//! Part of the HUD (hud.rs), the UI thread's.

use std::collections::HashSet;

use engine::core::math::noise::perlin2;
use engine::prelude::*;
use engine_ui::Color;
use engine_ui::draw::DrawCtx;
use engine_ui::el::El;
use engine_ui::kit::brass::{self, Colors};
use engine_ui::node::Rect;
use engine_ui::style::{Align, Direction, Justify, Pick, Position, Shadow};
use engine_ui::text::{DISPLAY_FONT, TextSpec};

use crate::camera::{self, StrategyCam};
use crate::defs::Kind;
use crate::fleet::Vessel;
use crate::grid::{self, Grid};
use crate::island;
use crate::sim::Building;
use crate::vessels::Craft;

/// The world the chart covers, meters (X right, Z down the chart: north is up).
const X0: f64 = -3000.0;
const X1: f64 = 3300.0;
const Z0: f64 = -2500.0;
const Z1: f64 = 3400.0;
/// The painted picture's size, pixels.
pub(super) const PW: usize = 504;
pub(super) const PH: usize = 472;
/// The chart node's size, logical px (same aspect as the picture).
pub const CHART_W: f32 = 300.0;
pub const CHART_H: f32 = CHART_W * PH as f32 / PW as f32;

pub(super) const PICTURE: &str = "isle_chart";
pub const PAINTER: &str = "isle_map";

/// Chart position 0..1 of a world point.
pub(super) fn to_uv(x: f64, z: f64) -> Vec2 {
    Vec2::new(((x - X0) / (X1 - X0)) as f32, ((z - Z0) / (Z1 - Z0)) as f32)
}

fn from_uv(uv: Vec2) -> DVec2 {
    DVec2::new(X0 + uv.x as f64 * (X1 - X0), Z0 + uv.y as f64 * (Z1 - Z0))
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn rgb(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0,
        (hex & 255) as f32 / 255.0,
    ]
}

fn hash(x: i64, y: i64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 29;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 32;
    (h & 0xFFFF) as f32 / 65535.0
}

/// One value per chart pixel, computed on every core.
fn sample(f: impl Fn(f64, f64) -> f32 + Sync) -> Vec<f32> {
    let mut out = vec![0f32; PW * PH];
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16);
    let rows_per = PH.div_ceil(threads);
    std::thread::scope(|s| {
        for (k, chunk) in out.chunks_mut(rows_per * PW).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, v) in chunk.iter_mut().enumerate() {
                    let (x, y) = (i % PW, k * rows_per + i / PW);
                    let p = from_uv(Vec2::new(
                        (x as f32 + 0.5) / PW as f32,
                        (y as f32 + 0.5) / PH as f32,
                    ));
                    *v = f(p.x, p.y);
                }
            });
        }
    });
    out
}

/// Paint the chart: straight-alpha sRGB RGBA8, `PW`×`PH`.
pub fn paint_chart() -> Vec<u8> {
    let h = sample(|x, z| island::height(x, z) as f32);
    let w = sample(|x, z| {
        if island::height(x, z) > 2.0 {
            island::woods(x, z) as f32
        } else {
            0.0
        }
    });
    let at = |x: i64, y: i64| {
        h[(y.clamp(0, PH as i64 - 1) as usize) * PW + x.clamp(0, PW as i64 - 1) as usize]
    };
    let crosses = |x: i64, y: i64, level: f32, reach: i64| {
        let s = at(x, y) > level;
        (1..=reach).any(|r| {
            (at(x + r, y) > level) != s
                || (at(x, y + r) > level) != s
                || (at(x - r, y) > level) != s && r > 1
                || (at(x, y - r) > level) != s && r > 1
        })
    };
    let (paper_a, paper_b) = (rgb(0xf1e6c8), rgb(0xe3d2a6));
    let (sea_shallow, sea_deep) = (rgb(0xc9dccb), rgb(0x8fb0ae));
    let (land_low, land_high) = (rgb(0xdcc792), rgb(0xae9461));
    let sand = rgb(0xead7a3);
    let wood_tint = rgb(0x9fa86c);
    let tree = rgb(0x4d6a35);
    let ink = rgb(0x3a2814);
    let sound = rgb(0x5f8588);
    let grid_ink = rgb(0x8a6a3f);
    let aged = rgb(0xa77a42);
    let mut px = vec![0u8; PW * PH * 4];
    let m_per_px = (X1 - X0) / PW as f64;
    for y in 0..PH as i64 {
        for x in 0..PW as i64 {
            let i = y as usize * PW + x as usize;
            let hv = h[i];
            let p = from_uv(Vec2::new(
                (x as f32 + 0.5) / PW as f32,
                (y as f32 + 0.5) / PH as f32,
            ));
            // Paper: mottled, with fibres.
            let mott = perlin2(p.x / 900.0, p.y / 900.0, 5) * 0.5 + 0.5;
            let fibre = hash(x / 3, y) * 0.5 + hash(x, y / 3) * 0.5;
            let mut c = mix(paper_a, paper_b, mott * 0.8 + fibre * 0.2);
            if hv <= 0.0 {
                // The sea, washed over the paper, deeper further out.
                let t = (-hv / 40.0).clamp(0.0, 1.0).powf(0.6);
                c = mix(c, mix(sea_shallow, sea_deep, t), 0.75);
                for (k, level) in [-1.5f32, -4.0, -9.0, -18.0].into_iter().enumerate() {
                    if crosses(x, y, level, 1) {
                        c = mix(c, sound, 0.55 - k as f32 * 0.1);
                    }
                }
            } else {
                // Land: warmer and darker uphill, lit from the north-west.
                c = mix(land_low, land_high, (hv / 45.0).clamp(0.0, 1.0));
                c = mix(c, paper_a, 0.15 + 0.15 * mott);
                let gx = at(x + 1, y) - at(x - 1, y);
                let gz = at(x, y + 1) - at(x, y - 1);
                let shade = (1.0 + (gx + gz) * 0.06).clamp(0.72, 1.18);
                c = [c[0] * shade, c[1] * shade, c[2] * shade];
                if hv < 2.2 {
                    c = mix(c, sand, 0.8);
                }
                let wv = w[i];
                c = mix(c, wood_tint, wv * 0.55);
                // Stippled trees on a jittered 4 px lattice.
                let (cx, cy) = (x.div_euclid(4), y.div_euclid(4));
                let jx = cx as f32 * 4.0 + 1.0 + hash(cx, cy) * 2.0;
                let jy = cy as f32 * 4.0 + 1.0 + hash(cy, cx + 77) * 2.0;
                let d = ((x as f32 - jx).powi(2) + (y as f32 - jy).powi(2)).sqrt();
                if wv > 0.3 && d < 1.25 && hash(cx + 13, cy * 3) < wv * 1.2 {
                    c = mix(c, tree, 0.85);
                }
                // Contours every 10 m.
                for level in [10.0f32, 20.0, 30.0, 40.0] {
                    if crosses(x, y, level, 1) {
                        c = mix(c, grid_ink, 0.35);
                    }
                }
            }
            // The coast, inked twice as thick.
            if crosses(x, y, 0.0, 1) {
                c = mix(c, ink, 0.92);
            }
            // A graticule every kilometre.
            let (wx, wz) = (p.x.rem_euclid(1000.0), p.y.rem_euclid(1000.0));
            if wx < m_per_px || wz < m_per_px {
                c = mix(c, grid_ink, 0.22);
            }
            // Aged toward the edges.
            let (u, v) = (x as f32 / PW as f32, y as f32 / PH as f32);
            let edge = (u.min(1.0 - u).min(v).min(1.0 - v) * 9.0).clamp(0.0, 1.0);
            c = mix(c, aged, (1.0 - edge) * 0.45 + (1.0 - mott) * 0.06);
            let o = i * 4;
            for k in 0..3 {
                px[o + k] = (c[k].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
            px[o + 3] = 255;
        }
    }
    px
}

/// Paint the chart and add the painter. A few hundred milliseconds on four cores.
pub fn install(world: &mut World) {
    let started = std::time::Instant::now();
    let pixels = paint_chart();
    if let Some(mut lib) = world.get_resource_mut::<engine_ui::icons::IconLibrary>() {
        lib.add_picture_rgba(PICTURE, PW as u32, PH as u32, pixels);
    }
    if let Some(mut p) = world.get_resource_mut::<engine_ui::Painters>() {
        p.add(PAINTER, paint);
    }
    engine::core::log::info!(
        "archipelago: map table painted in {:.0} ms",
        started.elapsed().as_secs_f32() * 1000.0
    );
}

/// The panel: a title with its medallions over the framed chart.
pub fn panel() -> El {
    let header = El::new()
        .class("row")
        .with_style(|s| s.gap(10.0).align_items(Align::Center).justify(Justify::SpaceBetween))
        .child(
            El::new()
                .class("col")
                .with_style(|s| s.gap(0.0))
                .child(
                    El::text("{isle.island}")
                        .class("brass_title")
                        .text_style(|t| t.nowrap().shadow().size(18.0))
                        .pick_ignore(),
                )
                .child(
                    El::text("{isle.map_note}")
                        .class("brass_label")
                        .text_style(|t| t.nowrap())
                        .pick_ignore(),
                ),
        )
        .child(
            El::new()
                .class("row")
                .with_style(|s| s.gap(8.0))
                .child(
                    brass::medallion("isle_ledger", 38.0, "isle.ledger")
                        .id("isle.ledger")
                        .tooltip("Ledger", "Goods made and used, the treasury and the people, island by island.\n[b]Key[/b]  L"),
                )
                .child(
                    brass::medallion("isle_routes", 38.0, "isle.routes")
                        .tooltip("Trade routes", "Every route, its stops and cargo, and its ships on the chart.\n[b]Key[/b]  T"),
                )
                .child(
                    brass::medallion("isle_home", 38.0, "isle.home")
                        .tooltip("Home", "Back to the town on Haverholm.\n[b]Key[/b]  H"),
                ),
        );
    let chart = El::new()
        .id("isle.map")
        .on_click("isle.map")
        .paint(PAINTER)
        .tooltip("Sea chart", "Click to look there.")
        .with_style(|s| {
            s.size(CHART_W, CHART_H)
                .radius(2.0)
                .border(1.0, Color::hex(0x2a1a0a))
                .inset_shadow(Shadow::new(Color::hexa(0x3a220a90), 14.0, 0.0))
                .pick(Pick::Block)
        });
    let plate = brass::plate().with_style(|s| {
        s.direction(Direction::Column)
            .gap(10.0)
            .padding([14.0, 16.0, 16.0, 16.0])
            .enter(engine_ui::style::Enter::rise(0.45, 40.0).delay(0.1))
    });
    El::new()
        .with_style(|s| s.fill_parent().pick(Pick::Ignore))
        .child(
            El::new()
                .with_style(|s| s.position(Position::Absolute).right(24.0).bottom(24.0))
                .child(plate.child(header).child(chart)),
        )
}

/// Where in the world a pointer on the chart node `e` points, if it is on it.
fn world_under(world: &World, e: Entity, pointer: Vec2) -> Option<DVec2> {
    let r = world.get::<engine_ui::node::ComputedNode>(e)?.rect;
    let uv = Vec2::new((pointer.x - r.x) / r.w, (pointer.y - r.y) / r.h);
    ((0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y)).then(|| from_uv(uv))
}

/// Send the view to a world point (the camera glides there).
pub fn look_at(world: &mut World, p: DVec2) {
    let y = island::height(p.x, p.y).max(0.0);
    let mut q = world.query::<&mut StrategyCam>();
    for mut cam in q.iter_mut(world) {
        cam.goal_focus = DVec3::new(p.x, y, p.y);
    }
}

/// The chart was clicked.
pub fn clicked(world: &mut World, e: Option<Entity>) {
    let e = e.or_else(|| engine_ui::input::find_id(world, "isle.map"));
    let pointer = world.resource::<engine::input::Mouse>().position;
    if let (Some(e), Some(p)) = (e, pointer)
        && let Some(at) = world_under(world, e, p)
    {
        look_at(world, at);
    }
}

/// Back to the home town.
pub fn home(world: &mut World) {
    look_at(world, island::home_town_spot());
}

/// The ground corners of the view (top-left, top-right, bottom-right, bottom-left), on the
/// sea plane, a few kilometres at most.
fn view_corners(world: &mut World) -> Option<[DVec2; 4]> {
    let window = world.resource::<engine::core::window::Window>().clone();
    let mut q = world.query::<(&Transform, &Camera, &StrategyCam)>();
    let (t, cam, sc) = q.iter(world).next()?;
    let (w, h) = (window.width as f32, window.height as f32);
    let focus = DVec2::new(sc.focus.x, sc.focus.z);
    let mut out = [DVec2::ZERO; 4];
    for (k, s) in [
        Vec2::new(0.0, 0.0),
        Vec2::new(w, 0.0),
        Vec2::new(w, h),
        Vec2::new(0.0, h),
    ]
    .into_iter()
    .enumerate()
    {
        let (o, d) = camera::cursor_ray(t, cam, &window, s);
        let d = d.as_dvec3();
        let reach = 4000.0;
        let p = if d.y < -1e-3 {
            let t = (-o.y / d.y).min(reach);
            o + d * t
        } else {
            o + DVec3::new(d.x, 0.0, d.z).normalize_or_zero() * reach
        };
        let p = DVec2::new(p.x, p.z);
        // Keep far corners from running off when the view looks at the horizon.
        let off = p - focus;
        out[k] = focus + off.clamp_length_max(reach);
    }
    Some(out)
}

fn paint(world: &mut World, _e: Entity, ctx: &mut DrawCtx) {
    let r = ctx.rect;
    let px = ctx.px;
    let at = |x: f64, z: f64| {
        let uv = to_uv(x, z);
        Vec2::new(r.x + uv.x * r.w, r.y + uv.y * r.h)
    };
    ctx.icon(PICTURE, r, Color::WHITE);
    // Roads as a fine brown hatching, one dot per chart pixel.
    let road = Color::hexa(0x6b4a26c0);
    let roads: HashSet<(i32, i32)> = world.get_resource::<Grid>().map_or_else(HashSet::new, |g| {
        g.roads
            .iter()
            .map(|c| {
                let p = grid::cell_center(*c);
                let q = at(p.x, p.y);
                (q.x.floor() as i32, q.y.floor() as i32)
            })
            .collect()
    });
    for (x, y) in roads {
        ctx.rect(
            Rect::new(x as f32, y as f32, 1.0 * px.max(1.0), 1.0 * px.max(1.0)),
            road,
            0.0,
            0.0,
            Color::TRANSPARENT,
        );
    }
    // Buildings: brick-red homes, umber workshops, slate-blue harbours.
    let mut q = world.query::<&Building>();
    let marks: Vec<(Vec2, Color, f32)> = q
        .iter(world)
        .filter(|b| b.kind != Kind::Road)
        .map(|b| {
            let p = grid::cell_center(b.origin);
            let (c, s) = match b.kind {
                Kind::House => (Color::hex(0xa13d22), 2.2),
                Kind::Harbour => (Color::hex(0x2c4a66), 3.6),
                Kind::Chapel | Kind::Market => (Color::hex(0x5c2a6a), 3.0),
                _ => (Color::hex(0x4a3016), 2.8),
            };
            (at(p.x, p.y), c, s)
        })
        .collect();
    for (p, c, s) in marks {
        let s = s * px;
        ctx.rect(
            Rect::new(p.x - s * 0.5, p.y - s * 0.5, s, s),
            c,
            0.5 * px,
            0.0,
            Color::TRANSPARENT,
        );
    }
    // Island names in the display serif.
    for isle in island::islands() {
        let mut p = at(isle.center.x, isle.center.y + isle.radius * 1.05);
        let spec = TextSpec {
            text: isle.name.to_string(),
            size: 12.5 * px,
            weight: 700,
            italic: false,
            color: Color::hexa(0x2a1a0ae0),
            family: Some(DISPLAY_FONT.into()),
            wrap: false,
            ..Default::default()
        };
        // Keep the name on the chart.
        let half = ctx.measure(&spec).x * 0.5 + 4.0 * px;
        p.x = p.x.clamp(r.x + half, r.x + r.w - half);
        ctx.text_spec(&spec, p, Vec2::new(0.5, 0.0));
    }
    // Ships and airships.
    let mut q = world.query::<(&Vessel, &Transform)>();
    let vessels: Vec<(Vec2, f32, bool)> = q
        .iter(world)
        .map(|(v, t)| {
            (
                at(t.translation.x, t.translation.z),
                std::f32::consts::FRAC_PI_2 - v.heading,
                v.craft == Craft::Airship,
            )
        })
        .collect();
    for (p, angle, air) in vessels {
        let s = if air { 15.0 } else { 12.0 } * px;
        let r = Rect::new(p.x - s * 0.5, p.y - s * 0.5, s, s);
        ctx.icon_rotated(
            if air {
                "isle_mark_airship"
            } else {
                "isle_mark_ship"
            },
            r,
            Color::WHITE,
            angle,
        );
    }
    // The view: a brass trapezoid with a glow where it looks.
    if let Some(c) = view_corners(world) {
        let pts: Vec<Vec2> = c.iter().map(|p| at(p.x, p.y)).collect();
        for k in 0..4 {
            let (a, b) = (pts[k], pts[(k + 1) % 4]);
            ctx.line(a, b, 3.2 * px, Color::hexa(0x1a0e0490), 0.0);
            ctx.line(a, b, 1.6 * px, Colors::brass_light(), 0.0);
        }
    }
    let focus = {
        let mut q = world.query::<&StrategyCam>();
        q.iter(world).next().map(|c| c.focus)
    };
    if let Some(f) = focus {
        let p = at(f.x, f.z);
        ctx.glow(p, 9.0 * px, Colors::brass_light().with_alpha(0.55), 2.0);
        ctx.rect(
            Rect::new(p.x - 2.0 * px, p.y - 2.0 * px, 4.0 * px, 4.0 * px),
            Colors::cream(),
            2.0 * px,
            1.0,
            Color::hex(0x2a1a0a),
        );
    }
    // A compass rose in the open sea at the top right.
    let s = 54.0 * px;
    ctx.icon(
        "isle_compass",
        Rect::new(r.x + r.w - s - 8.0 * px, r.y + 8.0 * px, s, s),
        Color::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_covers_every_island_and_round_trips() {
        for isle in island::islands() {
            for d in [DVec2::X, DVec2::NEG_X, DVec2::Y, DVec2::NEG_Y] {
                let p = isle.center + d * isle.radius * 1.3;
                let uv = to_uv(p.x, p.y);
                assert!(
                    (0.02..0.98).contains(&uv.x) && (0.02..0.98).contains(&uv.y),
                    "{} at {uv:?}",
                    isle.name
                );
                assert!((from_uv(uv) - p).length() < 0.01);
            }
        }
    }
}
