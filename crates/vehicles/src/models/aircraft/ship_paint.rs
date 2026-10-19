//! The spaceplane's textures: black tiles (per-tile tone, gaps, bevels), white tiles,
//! quilted blankets, carbon leading edges, windows, hatch, port plates, gear wells, payload
//! bay door lines, stencils; plus the engine bells' cooling tubes, a tiling tile set for the
//! gear doors and the base heat shield's quilting.

use engine_core::math::{Vec2, Vec3};
use engine_render::Image;

use super::ship_build::{MAIN_WELL, NOSE_WELL};
use super::ship_parts::{HATCH, HATCH_R, HOVER_FLANGE, WINDOWS};
use super::spaceplane::*;
use super::toolkit::*;

/// Where the painters put plates and markings.
pub struct Layout {
    /// RCS ports on the hull (body frame): point, normal.
    pub hull_ports: Vec<(Vec3, Vec3)>,
    /// RCS ports on the right OMS pod (pod frame).
    pub oms_ports: Vec<(Vec3, Vec3)>,
    /// RCS ports on the right wingtip pod (pod frame).
    pub tip_ports: Vec<(Vec3, Vec3)>,
    /// Hover engine centers (x, z).
    pub hover: Vec<Vec2>,
}

fn mix(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

/// Coverage of `v` in `a..b`, antialiased by `aa`.
fn band(v: f32, a: f32, b: f32, aa: f32) -> f32 {
    let aa = aa.max(1e-6);
    ((v - a) / aa + 0.5).clamp(0.0, 1.0) * ((b - v) / aa + 0.5).clamp(0.0, 1.0)
}

/// Black heat-shield tiles on a grid of `size` meters in `coord`.
fn black_tile(paint: &mut Paint, coord: Vec2, size: f32, texel: f32, seed: u32) {
    let g = coord / size;
    let cell = g.floor();
    let f = g - cell;
    let (ix, iy) = (cell.x as i32, cell.y as i32);
    let r = rand(ix, iy, 1, seed);
    let r2 = rand(ix, iy, 2, seed);
    let edge = f.x.min(1.0 - f.x).min(f.y).min(1.0 - f.y) * size;
    let mut tone = 0.020 + 0.020 * r;
    if r2 > 0.975 {
        // A replaced tile, still fresh.
        tone = 0.010;
    } else if r2 < 0.02 {
        tone = 0.055;
    }
    // Coating texture and a little scorching toward one corner.
    // Coating grain about a texel across (a hash per cell is plenty at that size).
    let grain = rand(
        (coord.x * 90.0).floor() as i32,
        (coord.y * 90.0).floor() as i32,
        7,
        seed,
    );
    tone *= 0.85 + 0.3 * grain;
    paint.color = Vec3::new(tone, tone * 0.98, tone * 0.97);
    paint.rough = 0.78 + 0.1 * r;
    paint.metal = 0.0;
    // Bevel and gap.
    paint.height =
        -0.0012 * (1.0 - smoothstep(0.0, 0.007, edge)) + 0.0004 * (r - 0.5) + 0.00015 * grain;
    let gap = line(edge, 0.0009, texel);
    paint.height -= 0.002 * gap;
    paint.color = mix(paint.color, Vec3::splat(0.004), gap);
    paint.ao = 1.0 - 0.5 * gap;
    paint.glow = 0.75 + 0.25 * r;
}

/// White tiles (larger, lighter).
fn white_tile(paint: &mut Paint, coord: Vec2, size: f32, texel: f32, seed: u32) {
    let g = coord / size;
    let cell = g.floor();
    let f = g - cell;
    let r = rand(cell.x as i32, cell.y as i32, 3, seed);
    let edge = f.x.min(1.0 - f.x).min(f.y).min(1.0 - f.y) * size;
    let grain = rand(
        (coord.x * 60.0).floor() as i32,
        (coord.y * 60.0).floor() as i32,
        3,
        seed,
    );
    let tone = 0.60 + 0.08 * r - 0.04 * grain;
    paint.color = Vec3::new(tone, tone * 0.985, tone * 0.95);
    paint.rough = 0.72 + 0.08 * r;
    paint.metal = 0.0;
    paint.height = -0.001 * (1.0 - smoothstep(0.0, 0.006, edge)) + 0.0003 * (r - 0.5);
    let gap = line(edge, 0.0008, texel);
    paint.height -= 0.0015 * gap;
    paint.color = mix(paint.color, Vec3::splat(0.12), gap * 0.8);
    paint.ao = 1.0 - 0.4 * gap;
    paint.glow = 0.2;
}

/// Quilted blankets: panels of `panel` meters stitched into small pillows.
fn blanket(paint: &mut Paint, coord: Vec2, texel: f32, seed: u32) {
    let panel = 0.76;
    let g = coord / panel;
    let cell = g.floor();
    let f = g - cell;
    let r = rand(cell.x as i32, cell.y as i32, 4, seed);
    let seam = f.x.min(1.0 - f.x).min(f.y).min(1.0 - f.y) * panel;
    let q = coord / 0.05;
    let qf = q - q.floor();
    let qd = qf.x.min(1.0 - qf.x).min(qf.y).min(1.0 - qf.y) * 0.05;
    let pillow = smoothstep(0.0, 0.018, qd);
    let dirt = noise2(coord * 1.3, seed + 11);
    let tone = 0.70 + 0.05 * r - 0.10 * (dirt - 0.5).max(0.0);
    paint.color = Vec3::new(tone, tone * 0.975, tone * 0.91);
    paint.color *= 0.92 + 0.08 * pillow;
    paint.rough = 0.88;
    paint.metal = 0.0;
    paint.height = 0.0011 * pillow - 0.0006 * line(qd, 0.0012, texel);
    let s = line(seam, 0.0015, texel);
    paint.height -= 0.0015 * s;
    paint.color = mix(paint.color, paint.color * 0.72, s);
    paint.ao = 0.92 + 0.08 * pillow - 0.3 * s;
    paint.glow = 0.08;
}

/// Reinforced carbon on the nose cap and leading edges: dark grey, satin, panel seams across
/// `along` every `pitch` meters.
fn carbon(paint: &mut Paint, along: f32, pitch: f32, texel: f32, seed: u32) {
    let g = along / pitch;
    let cell = g.floor();
    let r = rand(cell as i32, 0, 5, seed);
    let tone = 0.10 + 0.02 * r;
    paint.color = Vec3::new(tone, tone * 1.01, tone * 1.04);
    paint.rough = 0.6 + 0.08 * r;
    paint.metal = 0.0;
    let seam = (g - cell).min(1.0 - (g - cell)) * pitch;
    let s = line(seam, 0.0012, texel);
    paint.height = -0.0015 * s;
    paint.color = mix(paint.color, Vec3::splat(0.01), s);
    paint.glow = 1.0;
}

/// A dark plate with rounded corners under an RCS port (`local` in the port's tangent plane).
fn port_plate(paint: &mut Paint, local: Vec2, texel: f32) {
    let d = rounded_rect(local, Vec2::splat(0.11), 0.02);
    if d < texel {
        let k = (-d / texel + 0.5).clamp(0.0, 1.0);
        paint.color = mix(paint.color, Vec3::splat(0.03), k);
        paint.rough = paint.rough + (0.55 - paint.rough) * k;
        paint.glow = paint.glow.max(k);
        paint.height -= 0.001 * k;
        let rim = line(d, 0.0015, texel);
        paint.color = mix(paint.color, Vec3::splat(0.25), rim * 0.6);
    }
    // Scorching downstream of the nozzles is painted by the callers.
}

/// Tangent-plane coordinates of `p` around a port (`c`, normal `n`).
fn port_local(p: Vec3, c: Vec3, n: Vec3) -> Option<Vec2> {
    let d = p - c;
    if d.length_squared() > 0.09 || d.dot(n).abs() > 0.06 {
        return None;
    }
    let t1 = n
        .cross(Vec3::Z)
        .normalize_or(n.cross(Vec3::X).normalize_or(Vec3::Y));
    let t2 = n.cross(t1);
    Some(Vec2::new(d.dot(t1), d.dot(t2)))
}

/// Stencil text at `q` in glyph units (`unit` meters each); returns coverage.
fn text(t: &StrokeText, q: Vec2, unit: f32, texel: f32) -> f32 {
    let d = t.distance(q) * unit;
    line(d, unit * 0.55, texel)
}

struct Row {
    hp: f32,
    chine: f32,
    /// RCS ports near this station (bits into the layout's hull ports).
    ports: u32,
}

/// The hull atlas.
pub fn paint_hull(atlas: &Atlas, h: &Hull, lay: &Layout) -> Baked {
    let mut rows: Vec<Row> = (0..atlas.h)
        .map(|_| Row {
            hp: 0.0,
            chine: 0.0,
            ports: 0,
        })
        .collect();
    par_rows(&mut rows, 1, |y, r| {
        let z = Z_NOSE + (y as f32 + 0.5) / atlas.h as f32 * HULL_UV;
        let (_, arcs) = h.ring(z.min(Z_BASE), HULL_N_LO, HULL_N_UP);
        let ports = lay
            .hull_ports
            .iter()
            .take(32)
            .enumerate()
            .filter(|(_, (c, _))| (c.z - z).abs() < 0.32)
            .fold(0u32, |m, (k, _)| m | (1 << k));
        r[0] = Row {
            hp: *arcs.last().unwrap_or(&0.0),
            chine: arcs[HULL_N_LO],
            ports,
        };
    });
    let name = StrokeText::new("KESTREL", 5.6);
    let rescue = StrokeText::new("RESCUE", 5.6);
    let danger = StrokeText::new("DANGER RCS", 5.6);
    let nose_gear = StrokeText::new("NOSE GEAR", 5.6);
    let reg = StrokeText::new("SP-01", 5.6);
    bake(atlas, true, 1.0, |t| {
        let p = t.p;
        let n = t.n;
        let texel = t.size;
        let z = p.z;
        let side = if t.uv.x >= 0.5 { 1.0 } else { -1.0 };
        let arc_b = (t.uv.x - 0.5).abs() * HULL_U;
        let row = &rows[((t.uv.y * atlas.h as f32) as usize).min(atlas.h - 1)];
        let a_top = row.hp - arc_b;
        let coord = Vec2::new(arc_b, z);
        let mut paint = Paint::new(Vec3::ONE, 0.8);
        // ---- zones
        let nose_cap = z < -10.15 + 0.12 * (arc_b - row.chine).clamp(0.0, 1.0);
        let window_zone = rounded_rect(
            Vec2::new(a_top, z) - Vec2::new(1.0, -6.75),
            Vec2::new(1.35, 1.05),
            0.35,
        );
        // Black below the chine, sweeping up the nose's sides; a black mask around the
        // windows; white tiles on the nose's top.
        let black_line = row.chine + 0.15 + 1.25 * smoothstep(-6.4, -10.1, z);
        let black = arc_b < black_line || window_zone < 0.0;
        if nose_cap {
            // Seams: a ring and radial panels.
            let ang = (p.y + 0.48).atan2(p.x);
            carbon(&mut paint, ang * 0.9, 0.42, texel, 1);
            let ring = line(
                z + 10.15 - 0.12 * (arc_b - row.chine).clamp(0.0, 1.0),
                0.0012,
                texel,
            );
            paint.height -= 0.0015 * ring;
        } else if black {
            black_tile(&mut paint, coord, 0.152, texel, 2);
        } else if p.y < 0.55 && a_top > 1.2 {
            white_tile(&mut paint, coord, 0.203, texel, 3);
        } else {
            blanket(&mut paint, Vec2::new(a_top, z), texel, 4);
            // Payload bay doors: center seam, hinge lines, ends, latch marks.
            if (-3.6..8.6).contains(&z) {
                let s = line(a_top, 0.002, texel).max(line(a_top - 2.05, 0.002, texel));
                paint.height -= 0.002 * s;
                paint.color = mix(paint.color, Vec3::splat(0.12), s);
                if a_top < 0.05 {
                    let latch = (z / 0.6).fract();
                    if latch < 0.05 {
                        paint.color *= 0.6;
                    }
                }
            }
            for zz in [-3.6f32, 2.5, 8.6] {
                if a_top < 2.05 {
                    let s = line(z - zz, 0.002, texel);
                    paint.height -= 0.002 * s;
                    paint.color = mix(paint.color, Vec3::splat(0.12), s);
                }
            }
        }
        // ---- windows (glossy black glass in a frame)
        if (-7.8..-5.6).contains(&z) && a_top < 2.1 {
            for &(c, half, r) in WINDOWS {
                let d = rounded_rect(Vec2::new(a_top, z) - c, half, r);
                if d < 0.04 {
                    let k = (-d / texel + 0.5).clamp(0.0, 1.0);
                    let glass = Vec3::new(0.004, 0.005, 0.007);
                    paint.color = mix(paint.color, glass, k);
                    paint.rough = paint.rough + (0.03 - paint.rough) * k;
                    paint.metal = 0.0;
                    paint.height = paint.height * (1.0 - k) - 0.004 * k;
                    paint.glow *= 1.0 - k;
                    paint.ao = paint.ao.max(k);
                }
            }
        }
        // ---- crew hatch (left side)
        if side < 0.0 && (z - HATCH.y).abs() < 0.9 {
            let q = Vec2::new(a_top - HATCH.x, z - HATCH.y);
            let d = q.length();
            let ring = line(d - HATCH_R, 0.002, texel);
            paint.height -= 0.003 * ring;
            paint.color = mix(paint.color, Vec3::splat(0.02), ring);
            if d < 0.11 {
                let k = band(d, -1.0, 0.11, texel);
                paint.color = mix(paint.color, Vec3::splat(0.006), k);
                paint.rough = paint.rough + (0.03 - paint.rough) * k;
            }
            // RESCUE with arrows above the hatch, and a cut-here outline around it.
            let unit = 0.06 / 6.0;
            let along = z - HATCH.y;
            let tq = Vec2::new(along / unit + rescue.width() * 0.5, (-q.x - 0.62) / unit);
            let k = text(&rescue, tq, unit, texel);
            paint.color = mix(paint.color, Vec3::new(0.85, 0.35, 0.05), k);
            let cut = line(d - HATCH_R - 0.06, 0.004, texel);
            let dash = f32::from(u8::from((q.y.atan2(q.x) * 18.0).sin() > 0.0));
            paint.color = mix(paint.color, Vec3::new(0.85, 0.35, 0.05), cut * dash);
        }
        // ---- name and registration (both sides, the white side tiles)
        if n.x.abs() > 0.75 && (-4.8..0.0).contains(&z) {
            let unit = 0.36 / 6.0;
            let along = if p.x > 0.0 { -(z + 2.6) } else { z + 2.6 };
            let q = Vec2::new(along / unit + name.width() * 0.5, (p.y - 0.62) / unit);
            let k = text(&name, q, unit, texel);
            paint.color = mix(paint.color, Vec3::splat(0.015), k);
        }
        if n.x.abs() > 0.75 && (6.0..9.0).contains(&z) {
            let unit = 0.20 / 6.0;
            let along = if p.x > 0.0 { -(z - 7.4) } else { z - 7.4 };
            let q = Vec2::new(along / unit + reg.width() * 0.5, (p.y - 0.55) / unit);
            let k = text(&reg, q, unit, texel);
            paint.color = mix(paint.color, Vec3::splat(0.015), k);
        }
        // ---- RCS port plates and their soot
        let mut near = row.ports;
        while near != 0 {
            let (c, cn) = lay.hull_ports[near.trailing_zeros() as usize];
            near &= near - 1;
            if let Some(local) = port_local(p, c, cn) {
                port_plate(&mut paint, local, texel);
                if cn.y.abs() < 0.5
                    && local.y.abs() > 0.13
                    && local.y.abs() < 0.24
                    && local.x.abs() < 0.2
                {
                    let unit = 0.025 / 6.0;
                    let tq = Vec2::new(
                        local.x / unit + danger.width() * 0.5,
                        (local.y.abs() - 0.15) / unit,
                    );
                    let k = text(&danger, tq, unit, texel);
                    paint.color = mix(paint.color, Vec3::new(0.8, 0.1, 0.05), k);
                }
            }
        }
        // ---- hover engine flanges (dark under the metal ring) and gear wells
        if p.y < -1.2 {
            for c in &lay.hover {
                let d = Vec2::new(p.x - c.x, z - c.y).length();
                if d < HOVER_FLANGE + 0.03 {
                    paint.color = Vec3::splat(0.02);
                    paint.glow = 0.5;
                }
                let door = rounded_rect(Vec2::new(p.x - c.x, z - c.y), Vec2::new(0.68, 0.72), 0.08);
                let s = line(door, 0.0015, texel);
                paint.height -= 0.002 * s;
                paint.color = mix(paint.color, Vec3::splat(0.003), s);
            }
            let mirror = Vec2::new(-1.0, 1.0);
            let wells = [NOSE_WELL, MAIN_WELL, (MAIN_WELL.0 * mirror, MAIN_WELL.1)];
            for (c, half) in wells {
                let d = rounded_rect(Vec2::new(p.x, z) - c, half, 0.06);
                if d < texel {
                    let k = (-d / texel + 0.5).clamp(0.0, 1.0);
                    paint.color = mix(paint.color, Vec3::splat(0.012), k);
                    paint.ao *= 1.0 - 0.8 * k;
                    paint.glow *= 1.0 - k;
                    paint.height -= 0.004 * k;
                }
                let s = line(d - 0.05, 0.0015, texel);
                paint.color = mix(paint.color, Vec3::splat(0.003), s);
            }
            // Nose gear stencil on the belly ahead of the well.
            let unit = 0.05 / 6.0;
            let ahead = NOSE_WELL.0.y - NOSE_WELL.1.y - 0.12;
            let tq = Vec2::new(-p.x / unit + nose_gear.width() * 0.5, (-(z - ahead)) / unit);
            let k = text(&nose_gear, tq, unit, texel);
            paint.color = mix(paint.color, Vec3::splat(0.6), k);
        }
        // ---- umbilical doors on the aft belly
        if p.y < -1.2 && z > 7.0 {
            for sx in [-1.0f32, 1.0] {
                let d = rounded_rect(Vec2::new(p.x - sx * 1.15, z - 8.3), Vec2::splat(0.42), 0.05);
                if d < texel {
                    let k = (-d / texel + 0.5).clamp(0.0, 1.0);
                    paint.color = mix(paint.color, paint.color * 1.6, k * 0.5);
                }
                let s = line(d, 0.0015, texel);
                paint.height -= 0.0025 * s;
                paint.color = mix(paint.color, Vec3::splat(0.002), s);
            }
        }
        // ---- vents and access panels on the white side tiles
        if n.x.abs() > 0.6 && n.y.abs() < 0.5 {
            for (vz, vy) in [(-5.6f32, 0.05f32), (-1.2, -0.25), (3.6, -0.3), (6.6, 0.2)] {
                let q = Vec2::new(z - vz, p.y - vy);
                let d = rounded_rect(q, Vec2::new(0.09, 0.055), 0.012);
                if d < texel {
                    let k = (-d / texel + 0.5).clamp(0.0, 1.0);
                    paint.color = mix(paint.color, Vec3::splat(0.01), k);
                    paint.rough = paint.rough + (0.5 - paint.rough) * k;
                    paint.height -= 0.003 * k;
                }
                let rim = line(d - 0.012, 0.0012, texel);
                paint.height -= 0.0012 * rim;
                let panel = rounded_rect(q - Vec2::new(0.0, 0.02), Vec2::new(0.24, 0.17), 0.02);
                let s = line(panel, 0.0009, texel);
                paint.height -= 0.0012 * s;
                paint.color = mix(paint.color, paint.color * 0.55, s);
            }
        }
        // ---- weathering: soot behind the nose cap on the belly, streaks along the flow
        if p.y < -0.9 && z > -10.2 {
            let streak = noise2(Vec2::new(p.x * 12.0, z * 0.25), 17);
            let k = smoothstep(-6.0, -10.0, z) * (0.25 + 0.5 * streak);
            paint.color = mix(paint.color, Vec3::new(0.05, 0.035, 0.025), k * 0.4);
        }
        paint
    })
}

/// The wing atlas: wings, fin, body flap, OMS and wingtip pods, the edge patch.
pub fn paint_wing(atlas: &Atlas, lay: &Layout) -> Baked {
    let no_step = StrokeText::new("NO STEP", 5.6);
    let reg = StrokeText::new("SP-01", 5.6);
    let wing = wing_panel();
    let fin = fin_panel();
    bake(atlas, true, 1.0, |t| {
        let p = t.p;
        let texel = t.size;
        let mut paint = Paint::new(Vec3::ONE, 0.8);
        match t.part {
            W_EDGE => {
                paint.color = Vec3::splat(0.03);
                paint.rough = 0.7;
                paint.glow = 0.8;
            }
            W_WING_LOWER | W_WING_UPPER => {
                let s = p.x;
                let from_le = p.z - (wing.le)(s);
                let lower = t.part == W_WING_LOWER;
                if from_le < 0.32 + 0.1 * smoothstep(3.5, 2.6, s) {
                    // Carbon leading edge, panels along the span.
                    carbon(&mut paint, s + 0.3, 0.55, texel, 6);
                    let tee = line(from_le - 0.30, 0.0015, texel);
                    paint.color = mix(paint.color, Vec3::splat(0.008), tee);
                } else if lower {
                    black_tile(&mut paint, Vec2::new(s, p.z), 0.152, texel, 8);
                    // The main gear well's outboard edge at the root.
                    let (c, half) = MAIN_WELL;
                    let d = rounded_rect(
                        Vec2::new(s, p.z) - Vec2::new(c.x + half.x - 0.15, c.y),
                        Vec2::new(0.15, half.y),
                        0.06,
                    );
                    let k = band(-d, 0.0, 9.0, texel);
                    paint.color = mix(paint.color, Vec3::splat(0.012), k);
                    paint.glow *= 1.0 - k;
                } else {
                    // Upper: white tiles inboard and forward, blankets elsewhere.
                    if s < 3.4 || p.z < 5.0 {
                        white_tile(&mut paint, Vec2::new(s, p.z), 0.203, texel, 9);
                    } else {
                        blanket(&mut paint, Vec2::new(s, p.z), texel, 10);
                    }
                    // NO STEP near the root.
                    let unit = 0.07 / 6.0;
                    let q = Vec2::new(
                        (p.z - 5.8) / unit + no_step.width() * 0.5,
                        (-(s - 2.85)) / unit,
                    );
                    let k = text(&no_step, q, unit, texel);
                    paint.color = mix(paint.color, Vec3::splat(0.05), k);
                }
                // Elevon hinge cover strip on top.
                let hinge = (wing.hinge)(s);
                if !lower {
                    let k = band(p.z, hinge - 0.30, hinge - 0.02, texel);
                    if k > 0.0 && s > 2.5 {
                        paint.color = mix(paint.color, Vec3::splat(0.55), k * 0.5);
                        let seam = line(p.z - hinge + 0.30, 0.0015, texel);
                        paint.height -= 0.0015 * seam;
                    }
                }
            }
            W_FIN_LEFT | W_FIN_RIGHT => {
                let y = p.x;
                let from_le = p.z - (fin.le)(y);
                if from_le < 0.22 {
                    carbon(&mut paint, y, 0.5, texel, 12);
                } else {
                    blanket(&mut paint, Vec2::new(y, p.z), texel, 13);
                    // Registration on both sides, reading forward to aft on the left and
                    // aft to forward on the right (always upright).
                    let left = t.part == W_FIN_LEFT;
                    let unit = 0.30 / 6.0;
                    let along = if left { p.z - 8.6 } else { -(p.z - 8.6) };
                    let q = Vec2::new(along / unit + reg.width() * 0.5, (y - 2.75) / unit);
                    let k = text(&reg, q, unit, texel);
                    paint.color = mix(paint.color, Vec3::splat(0.015), k);
                    // A thin dark band below the tip.
                    let k = band(y, 4.25, 4.33, texel);
                    paint.color = mix(paint.color, Vec3::splat(0.02), k);
                }
            }
            W_FLAP_LOWER | W_FLAP_UPPER => {
                black_tile(&mut paint, Vec2::new(p.x, p.z), 0.152, texel, 14);
            }
            W_OMS => {
                // Pod frame: centered on its axis.
                if p.z < 6.3 || p.y < -0.1 {
                    black_tile(&mut paint, Vec2::new(p.x + p.y, p.z), 0.152, texel, 15);
                } else {
                    blanket(&mut paint, Vec2::new(p.x + p.y * 0.5, p.z), texel, 16);
                }
                for &(c, cn) in &lay.oms_ports {
                    if let Some(local) = port_local(p, c, cn) {
                        port_plate(&mut paint, local, texel);
                    }
                }
            }
            W_TIP_POD => {
                carbon(&mut paint, p.z, 0.6, texel, 17);
                for &(c, cn) in &lay.tip_ports {
                    if let Some(local) = port_local(p, c, cn) {
                        port_plate(&mut paint, local, texel);
                    }
                }
            }
            _ => {}
        }
        paint
    })
}

/// Cooling tubes along a bell (u around, v along) and two hoops: normal map.
pub fn bell_normal() -> Image {
    normal_image(1024, 256, 3.4, 1.6, |u, v| {
        let f = (u * 200.0).fract();
        let tube = (1.0 - (2.0 * f - 1.0).powi(2)).max(0.0).sqrt();
        let hoop = [0.38f32, 0.62, 0.84]
            .iter()
            .map(|&c| (1.0 - ((v - c).abs() / 0.012).min(1.0)) * 0.0025)
            .fold(0.0f32, f32::max);
        0.003 * tube + hoop
    })
}

/// A tiling black-tile set (0.6 m square, 4 × 4 tiles) for the gear doors and other small
/// tiled parts.
pub fn tile_set() -> (Image, Image, Image) {
    let n = 256;
    let size = 0.6;
    let mut atlas = Atlas::new(n, n);
    // A flat quad covering the texture.
    let v = |u: f32, w: f32| {
        engine_render::Vertex::new(Vec3::new(u * size, w * size, 0.0), Vec3::Z, Vec2::new(u, w))
    };
    let quad = engine_render::Mesh::new(
        vec![v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)],
        vec![0, 1, 2, 0, 2, 3],
    );
    atlas.raster(&quad, 1);
    let b = bake(&atlas, false, 1.0, |t| {
        let mut paint = Paint::new(Vec3::ONE, 0.8);
        black_tile(&mut paint, Vec2::new(t.p.x, t.p.y), 0.15, t.size, 21);
        paint
    });
    (b.color, b.normal, b.orm)
}

/// The base heat shield's quilting (normal map, 2 m square).
pub fn base_quilt() -> Image {
    normal_image(512, 512, 2.0, 2.0, |u, v| {
        let q = Vec2::new(u, v) * 2.0 / 0.08;
        let f = q - q.floor();
        let d = f.x.min(1.0 - f.x).min(f.y).min(1.0 - f.y) * 0.08;
        0.002 * smoothstep(0.0, 0.025, d)
    })
}
