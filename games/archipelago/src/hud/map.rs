//! The map table: a region of the planet as an engraved sea chart on parchment, in a brass
//! frame at the bottom right, with arrows to turn to the other regions' charts (it follows
//! the camera's region otherwise). The chart itself is painted once from the islands' height
//! function (coasts inked, soundings as thin lines hugging the shore, hills shaded from the
//! north-west, woods stippled); towns, roads, ships, airships, island names and the part of
//! the world in view are drawn over it every frame. The fog of war is laid into the chart as
//! blank, hatched parchment with an inked edge round the known world, refreshed about once a
//! second when it moves. A click sends the view there.
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
use crate::globe;
use crate::grid::{self, Grid};
use crate::island;
use crate::sim::Building;
use crate::vessels::Craft;

/// The part of map space a region's chart covers, meters (X right, Z down the chart: north
/// is up), in the picture's aspect.
#[derive(Clone, Copy, Debug)]
pub(super) struct Bounds {
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
}

/// Every region's chart bounds: its islands with a margin of sea, widened to the picture's
/// aspect. (The home region keeps the frame it always had.)
pub(super) fn bounds(k: usize) -> Bounds {
    static ALL: std::sync::OnceLock<Vec<Bounds>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        (0..globe::regions().len())
            .map(|k| {
                if k == 0 {
                    return Bounds {
                        x0: -3000.0,
                        x1: 3300.0,
                        z0: -2500.0,
                        z1: 3400.0,
                    };
                }
                let (mut lo, mut hi) = (DVec2::splat(f64::MAX), DVec2::splat(f64::MIN));
                for isle in island::islands().iter().filter(|i| i.region() == k) {
                    let r = DVec2::splat(isle.radius * 1.45 + 400.0);
                    lo = lo.min(isle.center - r);
                    hi = hi.max(isle.center + r);
                }
                if lo.x > hi.x {
                    let o = globe::sheet_origin(k);
                    lo = o - DVec2::splat(5000.0);
                    hi = o + DVec2::splat(5000.0);
                }
                let aspect = PW as f64 / PH as f64;
                let (mut w, mut h) = (hi.x - lo.x, hi.y - lo.y);
                if w / h < aspect {
                    w = h * aspect;
                } else {
                    h = w / aspect;
                }
                let c = (lo + hi) * 0.5;
                Bounds {
                    x0: c.x - w * 0.5,
                    x1: c.x + w * 0.5,
                    z0: c.y - h * 0.5,
                    z1: c.y + h * 0.5,
                }
            })
            .collect()
    })[k.min(globe::regions().len() - 1)]
}

/// The painted picture's size, pixels.
pub(super) const PW: usize = 504;
pub(super) const PH: usize = 472;
/// The chart node's size, logical px (same aspect as the picture).
pub const CHART_W: f32 = 340.0;
pub const CHART_H: f32 = CHART_W * PH as f32 / PW as f32;

pub const PAINTER: &str = "isle_map";

/// The painted chart of region `k`, by name in the icon library.
pub(super) fn picture(k: usize) -> String {
    format!("isle_chart_{k}")
}

/// Chart position 0..1 of a map point on region `k`'s chart.
pub(super) fn to_uv(k: usize, x: f64, z: f64) -> Vec2 {
    let b = bounds(k);
    Vec2::new(
        ((x - b.x0) / (b.x1 - b.x0)) as f32,
        ((z - b.z0) / (b.z1 - b.z0)) as f32,
    )
}

fn from_uv(k: usize, uv: Vec2) -> DVec2 {
    let b = bounds(k);
    DVec2::new(
        b.x0 + uv.x as f64 * (b.x1 - b.x0),
        b.z0 + uv.y as f64 * (b.z1 - b.z0),
    )
}

/// The map table's state: which region's chart it shows, which charts are painted, and the
/// fog of war sampled over the chart.
#[derive(Resource, Default)]
pub struct MapTable {
    pub region: usize,
    /// Follow the region the camera is in (false once the player turns to another chart).
    pub pinned: bool,
    painted: Vec<bool>,
    /// Each region's chart as painted, before the fog is laid on (empty until painted).
    base: Vec<Vec<u8>>,
    /// Unseen share per fog cell (FOG×FOG over the chart), when it was sampled, and the fog
    /// and region last laid into the chart picture.
    fog: Vec<f32>,
    fog_age: f32,
    laid: Option<(usize, Vec<f32>)>,
}

const FOG: usize = 72;

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
fn sample(k: usize, f: impl Fn(f64, f64) -> f32 + Sync) -> Vec<f32> {
    let mut out = vec![0f32; PW * PH];
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16);
    let rows_per = PH.div_ceil(threads);
    std::thread::scope(|s| {
        for (part, chunk) in out.chunks_mut(rows_per * PW).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (i, v) in chunk.iter_mut().enumerate() {
                    let (x, y) = (i % PW, part * rows_per + i / PW);
                    let p = from_uv(
                        k,
                        Vec2::new((x as f32 + 0.5) / PW as f32, (y as f32 + 0.5) / PH as f32),
                    );
                    *v = f(p.x, p.y);
                }
            });
        }
    });
    out
}

/// Paint region `k`'s chart: straight-alpha sRGB RGBA8, `PW`×`PH`.
pub fn paint_chart(k: usize) -> Vec<u8> {
    let h = sample(k, |x, z| island::height(x, z) as f32);
    let w = sample(k, |x, z| {
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
    // Land and woods tinted by the region's climate.
    let (sea_shallow, sea_deep, land_low, land_high, wood_tint, tree) =
        match globe::regions()[k].biome {
            globe::Biome::Temperate => (0xc9dccb, 0x8fb0ae, 0xdcc792, 0xae9461, 0x9fa86c, 0x4d6a35),
            globe::Biome::Tropical => (0xb9ded0, 0x79aeb0, 0xd2cd8a, 0x9c9a58, 0x8fb062, 0x2f6a2c),
            globe::Biome::Arid => (0xc6d8c8, 0x8eaaa6, 0xe2bf86, 0xb07a4a, 0xb8a868, 0x6a6a30),
            globe::Biome::Arctic => (0xcfdde0, 0x94adb8, 0xe6e4da, 0xa8a8a0, 0x8e9c86, 0x3c5a48),
        };
    let (sea_shallow, sea_deep, land_low, land_high, wood_tint, tree) = (
        rgb(sea_shallow),
        rgb(sea_deep),
        rgb(land_low),
        rgb(land_high),
        rgb(wood_tint),
        rgb(tree),
    );
    let sand = rgb(0xead7a3);
    // Hills shade and contour by the region's own height.
    let top = island::islands()
        .iter()
        .filter(|i| i.region() == k)
        .map(|i| i.hills_m)
        .fold(45.0, f64::max) as f32;
    let step = (top / 4.0 / 10.0).round().max(1.0) * 10.0;
    let span = (bounds(k).x1 - bounds(k).x0) / 6300.0;
    let ink = rgb(0x3a2814);
    let sound = rgb(0x5f8588);
    let grid_ink = rgb(0x8a6a3f);
    let aged = rgb(0xa77a42);
    let mut px = vec![0u8; PW * PH * 4];
    let m_per_px = (bounds(k).x1 - bounds(k).x0) / PW as f64;
    let grid_m = if span > 1.5 { 2000.0 } else { 1000.0 };
    for y in 0..PH as i64 {
        for x in 0..PW as i64 {
            let i = y as usize * PW + x as usize;
            let hv = h[i];
            let p = from_uv(
                k,
                Vec2::new((x as f32 + 0.5) / PW as f32, (y as f32 + 0.5) / PH as f32),
            );
            // Paper: mottled, with fibres.
            let mott = perlin2(p.x / (900.0 * span), p.y / (900.0 * span), 5) * 0.5 + 0.5;
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
                c = mix(land_low, land_high, (hv / top).clamp(0.0, 1.0));
                c = mix(c, paper_a, 0.15 + 0.15 * mott);
                let gx = at(x + 1, y) - at(x - 1, y);
                let gz = at(x, y + 1) - at(x, y - 1);
                let shade = (1.0 + (gx + gz) * 0.06 / (span as f32).max(1.0)).clamp(0.72, 1.18);
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
                // Four contours up the region's hills.
                for level in [step, step * 2.0, step * 3.0, step * 4.0] {
                    if crosses(x, y, level, 1) {
                        c = mix(c, grid_ink, 0.35);
                    }
                }
            }
            // The coast, inked twice as thick.
            if crosses(x, y, 0.0, 1) {
                c = mix(c, ink, 0.92);
            }
            // A graticule every kilometre or two.
            let (wx, wz) = (p.x.rem_euclid(grid_m), p.y.rem_euclid(grid_m));
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

/// Paint the home chart and add the painter. A few hundred milliseconds on four cores; the
/// other regions' charts are painted the first time they are shown.
pub fn install(world: &mut World) {
    world.insert_resource(MapTable {
        painted: vec![false; globe::regions().len()],
        base: vec![Vec::new(); globe::regions().len()],
        ..Default::default()
    });
    ensure_painted(world, 0);
    if let Some(mut p) = world.get_resource_mut::<engine_ui::Painters>() {
        p.add(PAINTER, paint);
    }
}

/// Paint region `k`'s chart if it isn't yet.
pub(super) fn ensure_painted(world: &mut World, k: usize) {
    if world
        .get_resource::<MapTable>()
        .is_none_or(|m| m.painted.get(k).copied().unwrap_or(true))
    {
        return;
    }
    let started = std::time::Instant::now();
    let pixels = paint_chart(k);
    if let Some(mut lib) = world.get_resource_mut::<engine_ui::icons::IconLibrary>() {
        lib.add_picture_rgba(&picture(k), PW as u32, PH as u32, pixels.clone());
    }
    let mut m = world.resource_mut::<MapTable>();
    m.painted[k] = true;
    m.base[k] = pixels;
    if m.laid.as_ref().is_some_and(|l| l.0 == k) {
        m.laid = None;
    }
    engine::core::log::info!(
        "archipelago: chart of {} painted in {:.0} ms",
        globe::regions()[k].name,
        started.elapsed().as_secs_f32() * 1000.0
    );
}

/// The region the camera looks at.
fn camera_region(world: &mut World) -> usize {
    let mut q = world.query::<&StrategyCam>();
    q.iter(world)
        .next()
        .map_or(0, |c| globe::region_of(c.focus.x, c.focus.z))
}

/// Keep the chart on the camera's region (unless the player turned to another) and the fog
/// fresh. Called with the bars' refresh.
pub fn update(world: &mut World, dt: f32) {
    if !world.contains_resource::<MapTable>() {
        return;
    }
    let here = camera_region(world);
    let k = {
        let mut m = world.resource_mut::<MapTable>();
        if m.pinned && m.region == here {
            m.pinned = false;
        }
        if !m.pinned && m.region != here {
            m.region = here;
            m.fog_age = f32::MAX;
        }
        m.fog_age += dt;
        m.region
    };
    ensure_painted(world, k);
    if world.resource::<MapTable>().fog_age < 1.0 {
        return;
    }
    let fog: Vec<f32> = match world.get_resource::<crate::fog::FogOfWar>() {
        Some(f) => (0..FOG * FOG)
            .map(|i| {
                let uv = Vec2::new(
                    ((i % FOG) as f32 + 0.5) / FOG as f32,
                    ((i / FOG) as f32 + 0.5) / FOG as f32,
                );
                let p = from_uv(k, uv);
                (1.0 - f.seen_at(globe::dir(p.x, p.y))) as f32
            })
            .collect(),
        None => Vec::new(),
    };
    let mut m = world.resource_mut::<MapTable>();
    m.fog = fog;
    m.fog_age = 0.0;
    lay_fog(world, k);
}

/// The unseen share at chart pixel `(x, y)`, from the FOG×FOG cells (bilinear).
fn fog_at(fog: &[f32], x: usize, y: usize) -> f32 {
    if fog.len() != FOG * FOG {
        return 0.0;
    }
    let fx = ((x as f32 + 0.5) / PW as f32 * FOG as f32 - 0.5).clamp(0.0, (FOG - 1) as f32);
    let fy = ((y as f32 + 0.5) / PH as f32 * FOG as f32 - 0.5).clamp(0.0, (FOG - 1) as f32);
    let (i, j) = (fx as usize, fy as usize);
    let (i1, j1) = ((i + 1).min(FOG - 1), (j + 1).min(FOG - 1));
    let (tx, ty) = (fx - i as f32, fy - j as f32);
    let a = fog[j * FOG + i] * (1.0 - tx) + fog[j * FOG + i1] * tx;
    let b = fog[j1 * FOG + i] * (1.0 - tx) + fog[j1 * FOG + i1] * tx;
    a * (1.0 - ty) + b * ty
}

/// The chart `base` with the fog laid on: unseen water and land fade to blank parchment with
/// a fine diagonal hatching (the coasts still show faintly, as on an old, uncertain chart),
/// and the edge of the known world is inked in sepia.
pub(super) fn fogged(base: &[u8], fog: &[f32]) -> Vec<u8> {
    let mut out = base.to_vec();
    if fog.len() != FOG * FOG || base.len() != PW * PH * 4 {
        return out;
    }
    let blank = rgb(0xe4d5b0);
    let hatch = rgb(0xc9b58a);
    let ink = rgb(0x6e4c26);
    for y in 0..PH {
        for x in 0..PW {
            let raw = fog_at(fog, x, y);
            if raw < 0.03 {
                continue;
            }
            let u = {
                let t = ((raw - 0.12) / 0.7).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            };
            let o = (y * PW + x) * 4;
            let c = [
                base[o] as f32 / 255.0,
                base[o + 1] as f32 / 255.0,
                base[o + 2] as f32 / 255.0,
            ];
            let mottle = 0.96 + 0.06 * hash(x as i64 / 3, y as i64 / 3);
            let mut paper = blank.map(|v| v * mottle);
            if (x + y) % 6 == 0 {
                paper = mix(paper, hatch, 0.55);
            }
            let mut c = mix(c, paper, u * 0.86);
            // The inked edge of what has been seen.
            let edge = (1.0 - ((raw - 0.42).abs() * 7.0)).clamp(0.0, 1.0);
            c = mix(c, ink, edge * 0.42);
            for (n, v) in c.iter().enumerate() {
                out[o + n] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    }
    out
}

/// Lay the current fog into region `k`'s chart picture, when it moved since last time.
fn lay_fog(world: &mut World, k: usize) {
    let pixels = {
        let m = world.resource::<MapTable>();
        let same = m.laid.as_ref().is_some_and(|(r, f)| {
            *r == k
                && f.len() == m.fog.len()
                && f.iter().zip(&m.fog).all(|(a, b)| (a - b).abs() < 0.01)
        });
        if same || m.base[k].is_empty() {
            return;
        }
        fogged(&m.base[k], &m.fog)
    };
    if let Some(mut lib) = world.get_resource_mut::<engine_ui::icons::IconLibrary>() {
        lib.add_picture_rgba(&picture(k), PW as u32, PH as u32, pixels);
    }
    let mut m = world.resource_mut::<MapTable>();
    m.laid = Some((k, m.fog.clone()));
}

/// How much of the chart at `(x, z)` is still unseen, 0..1 (0 without a fog of war).
fn unseen(world: &World, x: f64, z: f64) -> f32 {
    world.get_resource::<MapTable>().map_or(0.0, |m| {
        let k = m.region;
        let uv = to_uv(k, x, z);
        let px = ((uv.x * PW as f32) as usize).min(PW - 1);
        let py = ((uv.y * PH as f32) as usize).min(PH - 1);
        fog_at(&m.fog, px, py)
    })
}

/// Turn the chart to the next (`step` 1) or previous (-1) region.
pub fn turn(world: &mut World, step: i32) {
    let n = globe::regions().len() as i32;
    let k = {
        let mut m = world.resource_mut::<MapTable>();
        m.region = (m.region as i32 + step).rem_euclid(n) as usize;
        m.pinned = true;
        m.fog_age = f32::MAX;
        m.region
    };
    ensure_painted(world, k);
    update(world, 0.0);
}

/// The region name and what it is like, for the panel's title.
pub fn region_label(world: &World) -> (&'static str, &'static str) {
    let k = world.get_resource::<MapTable>().map_or(0, |m| m.region);
    let r = &globe::regions()[k];
    (r.name, r.blurb)
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
                    El::new()
                        .class("row")
                        .with_style(|s| s.gap(6.0).align_items(Align::Center))
                        .child(
                            brass::medallion("isle_left", 24.0, "isle.map.prev")
                                .tooltip("", "The previous region's chart"),
                        )
                        .child(
                            // A fixed slot, so the panel keeps its size whichever region.
                            El::new()
                                .with_style(|s| s.width(140.0).justify(Justify::Center))
                                .child(
                                    El::text("{isle.map_region}")
                                        .class("brass_title")
                                        .text_style(|t| t.nowrap().shadow().size(16.0))
                                        .tooltip("{isle.map_region}", "{isle.map_blurb}"),
                                ),
                        )
                        .child(
                            brass::medallion("isle_speed_1", 24.0, "isle.map.next")
                                .tooltip("", "The next region's chart"),
                        ),
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
        .tooltip(
            "Sea chart",
            "Click to look there. Parchment shading is land and sea nobody has seen yet.",
        )
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
    let k = world.get_resource::<MapTable>().map_or(0, |m| m.region);
    let uv = Vec2::new((pointer.x - r.x) / r.w, (pointer.y - r.y) / r.h);
    ((0.0..=1.0).contains(&uv.x) && (0.0..=1.0).contains(&uv.y)).then(|| from_uv(k, uv))
}

/// Send the view to a world point (the camera glides there).
pub fn look_at(world: &mut World, p: DVec2) {
    let y = island::height(p.x, p.y).max(0.0);
    let mut q = world.query::<&mut StrategyCam>();
    for mut cam in q.iter_mut(world) {
        let far = globe::region_of(cam.focus.x, cam.focus.z) != globe::region_of(p.x, p.y);
        cam.goal_focus = DVec3::new(p.x, y, p.y);
        // Another region is half a world away: go there at once rather than glide.
        if far {
            cam.focus = cam.goal_focus;
        }
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
    let frame = *world.resource::<crate::globe::Frame>();
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
        let reach = 4000.0;
        // Where the corner's ray meets the round ground, as a map point.
        let p = camera::ground_hit(&frame, o, d).map_or(focus, |p| DVec2::new(p.x, p.z));
        // Keep far corners from running off when the view looks at the horizon.
        let off = p - focus;
        out[k] = focus + off.clamp_length_max(reach);
    }
    Some(out)
}

fn paint(world: &mut World, _e: Entity, ctx: &mut DrawCtx) {
    let r = ctx.rect;
    let px = ctx.px;
    let k = world.get_resource::<MapTable>().map_or(0, |m| m.region);
    let at = |x: f64, z: f64| {
        let uv = to_uv(k, x, z);
        Vec2::new(r.x + uv.x * r.w, r.y + uv.y * r.h)
    };
    let here = |x: f64, z: f64| globe::region_of(x, z) == k;
    ctx.icon(&picture(k), r, Color::WHITE);
    // Roads as a fine brown hatching, one dot per chart pixel.
    let road = Color::hexa(0x6b4a26c0);
    let roads: HashSet<(i32, i32)> = world.get_resource::<Grid>().map_or_else(HashSet::new, |g| {
        g.roads
            .iter()
            .filter(|c| {
                let p = grid::cell_center(**c);
                here(p.x, p.y)
            })
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
        .filter(|b| b.kind != Kind::Road && island::islands()[b.island].region() == k)
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
    let named: Vec<&island::Island> = island::islands()
        .iter()
        .filter(|i| i.region() == k && unseen(world, i.center.x, i.center.y) < 0.5)
        .collect();
    for isle in named {
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
    let mut q = world.query::<&Vessel>();
    let vessels: Vec<(Vec2, f32, bool)> = q
        .iter(world)
        .filter(|v| here(v.at.x, v.at.z))
        .map(|v| {
            (
                at(v.at.x, v.at.z),
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
    let looking_here = {
        let mut q = world.query::<&StrategyCam>();
        q.iter(world)
            .next()
            .is_some_and(|c| here(c.focus.x, c.focus.z))
    };
    if let Some(c) = view_corners(world).filter(|_| looking_here) {
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
    if let Some(f) = focus.filter(|f| here(f.x, f.z)) {
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
    // Nothing seen here yet: say so across the blank chart.
    let blank = world.get_resource::<MapTable>().is_some_and(|m| {
        m.fog.len() == FOG * FOG && m.fog.iter().sum::<f32>() / (FOG * FOG) as f32 > 0.85
    });
    if blank {
        let mid = Vec2::new(r.x + r.w * 0.5, r.y + r.h * 0.84);
        let title = TextSpec {
            text: "Uncharted waters".into(),
            size: 22.0 * px,
            weight: 700,
            italic: true,
            color: Color::hexa(0x4a3218d0),
            family: Some(DISPLAY_FONT.into()),
            wrap: false,
            ..Default::default()
        };
        ctx.text_spec(&title, mid, Vec2::new(0.5, 1.0));
        let note = TextSpec {
            text: "Send a ship or an airship to chart them".into(),
            size: 13.0 * px,
            weight: 500,
            italic: true,
            color: Color::hexa(0x5a4026c0),
            wrap: false,
            ..Default::default()
        };
        ctx.text_spec(&note, mid + Vec2::new(0.0, 6.0 * px), Vec2::new(0.5, 0.0));
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
            let k = isle.region();
            for d in [DVec2::X, DVec2::NEG_X, DVec2::Y, DVec2::NEG_Y] {
                let p = isle.center + d * isle.radius * 1.3;
                let uv = to_uv(k, p.x, p.y);
                assert!(
                    (0.02..0.98).contains(&uv.x) && (0.02..0.98).contains(&uv.y),
                    "{} at {uv:?}",
                    isle.name
                );
                assert!((from_uv(k, uv) - p).length() < 0.01);
            }
        }
    }
}
