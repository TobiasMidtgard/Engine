//! The sport plane's textures: livery, panel lines, fasteners, rivets, grime, the
//! registration, tire sidewalls and tread, seat leather and the cockpit screens.

use engine_core::math::{Vec2, Vec3};
use engine_render::Image;

use super::plane_parts::{BODY_U, BODY_V};
use super::sport_plane::*;
use super::toolkit::*;

pub const WHITE: u32 = 0xf1f1ed;
pub const NAVY: u32 = 0x15264d;
pub const ORANGE: u32 = 0xe8641c;
/// The fictional registration painted on the rear fuselage.
pub const REGISTRATION: &str = "Q-EVTX";

/// Atlas part ids in the body atlas.
pub const BODY_FUSELAGE: u8 = 1;
pub const BODY_FILLET: u8 = 2;
pub const BODY_PANT: u8 = 3;
pub const PATCH_WHITE: u8 = 9;
pub const PATCH_BLACK: u8 = 10;

/// A panel gap: darkens the paint and dips the height over `half` meters either side.
fn gap(paint: &mut Paint, d: f32, half: f32, size: f32, depth: f32) {
    let w = half.max(size * 0.55);
    let k = line(d, w, size);
    if k > 0.0 {
        paint.height -= depth * k;
        paint.color *= 1.0 - 0.62 * k;
        paint.ao *= 1.0 - 0.5 * k;
        paint.rough = paint.rough.max(0.5 * k + paint.rough * (1.0 - k));
    }
}

/// A row of dome rivets: `along` is the coordinate along the row, `across` the distance to
/// it (meters).
fn rivets(paint: &mut Paint, along: f32, across: f32, pitch: f32, r: f32, size: f32) {
    if across.abs() > r + size {
        return;
    }
    let t = along - (along / pitch).round() * pitch;
    let d = (t * t + across * across).sqrt();
    let rr = r.max(size * 0.7);
    if d < rr + size {
        let k = (1.0 - (d / rr).min(1.0).powi(2)).max(0.0).sqrt();
        paint.height += 0.00035 * k;
        // A thin dark ring where the head meets the skin.
        let ring = line(d - rr, size * 0.5, size);
        paint.color *= 1.0 - 0.07 * ring;
    }
}

/// A quarter-turn fastener: a flush disc with a ring groove and a slot.
fn fastener(paint: &mut Paint, local: Vec2, r: f32, size: f32) {
    let d = local.length();
    if d > r + 2.0 * size {
        return;
    }
    let ring = line(d - r, (size * 0.5).max(0.0005), size);
    paint.height -= 0.0006 * ring;
    paint.color *= 1.0 - 0.5 * ring;
    if d < r {
        paint.rough = 0.35;
        paint.color *= 0.86;
        let slot = line(local.y, (r * 0.18).max(size * 0.5), size)
            * (local.x.abs() < r * 0.75) as u8 as f32;
        paint.height -= 0.0005 * slot;
        paint.color *= 1.0 - 0.5 * slot;
    }
}

/// Screws around a rectangle's edge (local coordinates centered on it).
fn edge_screws(
    paint: &mut Paint,
    local: Vec2,
    half: Vec2,
    inset: f32,
    pitch: f32,
    r: f32,
    size: f32,
) {
    let inner = half - Vec2::splat(inset);
    let dx = local.x.abs() - inner.x;
    let dy = local.y.abs() - inner.y;
    if dx.abs() < r + size && local.y.abs() <= inner.y + r {
        rivets(paint, local.y + inner.y, dx, pitch, r, size);
    } else if dy.abs() < r + size && local.x.abs() <= inner.x + r {
        rivets(paint, local.x + inner.x, dy, pitch, r, size);
    }
}

/// Coverage of `value` inside `a..b`, antialiased by `aa`.
fn band(value: f32, a: f32, b: f32, aa: f32) -> f32 {
    let aa = aa.max(1e-6);
    ((value - a) / aa + 0.5).clamp(0.0, 1.0) * ((b - value) / aa + 0.5).clamp(0.0, 1.0)
}

fn mix(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

/// Per-station fuselage values (one texture row each).
struct Row {
    top: f32,
    bot: f32,
    e1: f32,
    e2: f32,
}

/// Livery band edges (fraction of the local fuselage height) along z.
fn livery_edges() -> (Curve, Curve) {
    let e1 = Curve::new(&[
        (-3.25, 0.47),
        (-2.6, 0.44),
        (-1.5, 0.47),
        (0.5, 0.50),
        (2.0, 0.54),
        (3.4, 0.61),
        (4.66, 0.70),
    ]);
    let e2 = Curve::new(&[
        (-3.25, 0.47),
        (-2.6, 0.56),
        (-1.5, 0.66),
        (0.5, 0.69),
        (2.0, 0.75),
        (3.4, 0.86),
        (4.66, 0.97),
    ]);
    (e1, e2)
}

/// The body atlas: fuselage, root fillets, wheel fairings and two plain patches.
pub fn paint_body(atlas: &Atlas, f: &Fuselage) -> Baked {
    let (e1c, e2c) = livery_edges();
    let rows: Vec<Row> = (0..atlas.h)
        .map(|y| {
            let z = Z_NOSE + (y as f32 + 0.5) / atlas.h as f32 * BODY_V;
            Row {
                top: f.top_at(z),
                bot: f.bottom_at(z),
                e1: e1c.at(z),
                e2: e2c.at(z),
            }
        })
        .collect();
    let inlets = super::plane_parts::inlets(f);
    let chin = super::plane_parts::chin_inlet(f);
    let reg = StrokeText::new(REGISTRATION, 5.6);
    let (white, navy, orange) = (hex(WHITE), hex(NAVY), hex(ORANGE));
    let black = hex(0x111111);
    bake(atlas, false, 1.0, |t| {
        let p = t.p;
        let n = t.n;
        let size = t.size;
        let mut paint = Paint::new(white, 0.17);
        match t.part {
            PATCH_WHITE => return paint,
            PATCH_BLACK => {
                paint.color = Vec3::splat(0.02);
                paint.rough = 0.8;
                return paint;
            }
            BODY_PANT => {
                // Two-tone fairing: navy below the axle line with an orange pinstripe.
                let c = f.bottom_at(0.45) * 0.0 - 0.62;
                let y = p.y - c;
                let k = band(y, -1.0, -0.055 + 0.06 * (p.z - 0.45), size);
                paint.color = mix(paint.color, navy, k);
                let s = band(
                    y,
                    -0.045 + 0.06 * (p.z - 0.45),
                    -0.033 + 0.06 * (p.z - 0.45),
                    size,
                );
                paint.color = mix(paint.color, orange, s);
                return paint;
            }
            BODY_FILLET => return paint,
            _ => {}
        }
        let z = p.z;
        let row = &rows[((t.uv.y * atlas.h as f32) as usize).min(atlas.h - 1)];
        let height = (row.top - row.bot).max(0.05);
        let h = (p.y - row.bot) / height;
        let aa = size / height;
        let side = n.x.abs();
        // ---- livery
        let nav = band(h, row.e1, row.e2, aa) * smoothstep(-3.3, -3.0, z);
        paint.color = mix(paint.color, navy, nav);
        let pin = band(h, row.e1 - 0.042, row.e1 - 0.024, aa) * smoothstep(-3.2, -2.7, z);
        paint.color = mix(paint.color, orange, pin);
        // Anti-glare panel ahead of the windshield.
        if (-2.2..Z_WS + 0.03).contains(&z) && n.y > 0.3 {
            let tz = ((z + 2.2) / (Z_WS + 2.2)).clamp(0.0, 1.0);
            let half = 0.15 + 0.19 * tz;
            let front = band(z, -2.15, 1.0, size);
            let k = band(p.x.abs(), -1.0, half, size) * front;
            paint.color = mix(paint.color, Vec3::splat(0.018), k);
            paint.rough = paint.rough + (0.72 - paint.rough) * k;
        }
        // Registration on both sides of the rear fuselage.
        if side > 0.55 && (1.4..3.3).contains(&z) {
            let unit = 0.185 / 6.0;
            let along = if p.x > 0.0 { -(z - 2.32) } else { z - 2.32 };
            let q = Vec2::new(along / unit + reg.width() * 0.5, (p.y + 0.165) / unit);
            let d = reg.distance(q) * unit;
            let k = line(d, unit * 0.62, size);
            paint.color = mix(paint.color, black, k);
        }
        // ---- seams and fasteners
        // Cowl to fuselage, cowl halves, firewall rivets.
        gap(&mut paint, z + 2.0, 0.0007, size, 0.0009);
        if (-1.995 - 0.004..-1.975 + 0.004).contains(&z) || (z + 1.985).abs() < 0.01 {
            rivets(&mut paint, t.uv.x * BODY_U, z + 1.985, 0.034, 0.0022, size);
        }
        if (-3.28..-2.0).contains(&z) && side > 0.25 {
            gap(&mut paint, p.y - 0.035, 0.0007, size, 0.0009);
            // Camlocs along the split line.
            let pitch = 0.115;
            let along = z + 2.06;
            let tz = along - (along / pitch).round() * pitch;
            fastener(&mut paint, Vec2::new(tz, p.y - 0.035 - 0.014), 0.0065, size);
        }
        if (-2.03..-1.97).contains(&z) {
            let pitch = 0.10;
            let along = t.uv.x * BODY_U;
            let tz = along - (along / pitch).round() * pitch;
            fastener(&mut paint, Vec2::new(tz, z + 2.0 + 0.016), 0.0065, size);
        }
        // Oil door on top of the cowl.
        if n.y > 0.5 && (-2.9..-2.3).contains(&z) {
            let local = Vec2::new(p.x, z + 2.62);
            let d = rounded_rect(local, Vec2::new(0.105, 0.13), 0.025);
            gap(&mut paint, d, 0.0007, size, 0.0008);
            let latch = rounded_rect(
                local - Vec2::new(0.0, -0.10),
                Vec2::new(0.025, 0.008),
                0.004,
            );
            gap(&mut paint, latch, 0.0005, size, 0.0004);
        }
        // Access panels: battery hatch (right side), belly inspection panel.
        if p.x > 0.0 && side > 0.5 && (3.05..3.7).contains(&z) {
            let local = Vec2::new(z - 3.37, p.y - 0.17);
            let half = Vec2::new(0.17, 0.10);
            gap(
                &mut paint,
                rounded_rect(local, half, 0.02),
                0.0007,
                size,
                0.0008,
            );
            edge_screws(&mut paint, local, half, 0.012, 0.05, 0.0024, size);
        }
        if n.y < -0.6 && (1.8..2.6).contains(&z) {
            let local = Vec2::new(p.x, z - 2.2);
            let half = Vec2::new(0.13, 0.25);
            gap(
                &mut paint,
                rounded_rect(local, half, 0.03),
                0.0007,
                size,
                0.0008,
            );
            edge_screws(&mut paint, local, half, 0.012, 0.055, 0.0024, size);
        }
        // Tail cone joint and the rivet line where the fin meets the turtle deck.
        gap(&mut paint, z - 3.62, 0.0006, size, 0.0007);
        if z > 3.6 && n.y > 0.6 && p.x.abs() < 0.08 {
            rivets(&mut paint, z, p.x.abs() - 0.055, 0.03, 0.0021, size);
        }
        // Static ports (both sides of the tail cone).
        if side > 0.6 && (3.05..3.15).contains(&z) {
            let d = Vec2::new(z - 3.1, p.y - 0.25).length();
            let ring = line(d - 0.009, 0.0012, size);
            paint.color = mix(paint.color, Vec3::splat(0.55), ring);
            if d < 0.0025 {
                paint.color = Vec3::splat(0.05);
            }
        }
        // ---- cowl openings
        for (c, cn, r) in inlets {
            let mut q = p - c;
            if q.dot(cn).abs() > 0.04 {
                continue;
            }
            q -= cn * q.dot(cn);
            let d = q.length();
            if d < r + size {
                let inside = ((r - d) / size + 0.5).clamp(0.0, 1.0);
                let depth = (d / r).min(1.0);
                // Dark duct with the engine's cooling fins faintly visible.
                let fins = (0.5 + 0.5 * (q.y * 260.0).sin()).powi(6) * (1.0 - depth);
                let inner = Vec3::splat(0.008 + 0.03 * depth * depth + 0.035 * fins);
                paint.color = mix(paint.color, inner, inside);
                paint.rough = paint.rough + (0.9 - paint.rough) * inside;
                paint.ao *= 1.0 - 0.7 * inside;
                paint.height -= 0.004 * inside;
            }
        }
        {
            let (c, cn, half) = chin;
            let mut q = p - c;
            let near = q.dot(cn).abs() < 0.04;
            q -= cn * q.dot(cn);
            let local = Vec2::new(q.x, q.y);
            let d = rounded_rect(local, half, half.y * 0.95);
            if near && d < size {
                let inside = (-d / size + 0.5).clamp(0.0, 1.0);
                let grille = (0.5 + 0.5 * (q.x * 300.0).sin()).powi(8);
                let inner = Vec3::splat(0.01 + 0.03 * grille);
                paint.color = mix(paint.color, inner, inside);
                paint.rough = paint.rough + (0.9 - paint.rough) * inside;
                paint.ao *= 1.0 - 0.7 * inside;
                paint.height -= 0.004 * inside;
            }
        }
        // ---- weathering
        if n.y < 0.15 && z > -2.15 {
            // Exhaust soot streaking back along the belly.
            let dz = z + 2.08;
            let spread = 0.07 + 0.07 * dz.max(0.0);
            let lateral = (-(p.x.abs() - 0.235).powi(2) / (spread * spread)).exp();
            let fade = (-dz.max(0.0) / 1.1).exp() * smoothstep(-2.2, -2.05, z);
            let streak = 0.55 + 0.45 * noise2(Vec2::new(p.x * 55.0, z * 1.6), 11);
            let k = (lateral * fade * streak * smoothstep(0.15, -0.6, n.y)).clamp(0.0, 1.0);
            paint.color = mix(paint.color, Vec3::new(0.11, 0.095, 0.085), k * 0.85);
            paint.rough = paint.rough + (0.7 - paint.rough) * k;
            // Thin oil streaks along the belly centerline behind the cowl.
            let oil = (-(p.x * p.x) / 0.012).exp()
                * (-(dz.max(0.0)) / 1.8).exp()
                * noise2(Vec2::new(p.x * 90.0, z * 0.8), 5).powi(3)
                * smoothstep(0.0, -0.8, n.y);
            paint.color = mix(
                paint.color,
                Vec3::new(0.30, 0.22, 0.12),
                (oil * 0.6).clamp(0.0, 1.0),
            );
        }
        // Barely visible large-scale unevenness in the clear coat.
        let wobble = noise2(Vec2::new(z * 3.0, t.uv.x * 12.0), 3);
        paint.rough += (wobble - 0.5) * 0.05;
        paint
    })
}

/// The wing atlas (right wing; the left is its mirror): white with navy tips, walkway,
/// fuel cap, inspection plates, landing lights, riveted control surfaces.
pub fn paint_wing(atlas: &Atlas) -> Baked {
    let (white, navy, orange) = (hex(WHITE), hex(NAVY), hex(ORANGE));
    bake(atlas, false, 1.0, |t| {
        let p = t.p;
        let n = t.n;
        let size = t.size;
        let mut paint = Paint::new(white, 0.17);
        if t.part == PATCH_BLACK {
            paint.color = Vec3::splat(0.03);
            paint.rough = 0.6;
            return paint;
        }
        let s = p.x;
        let c = wing_chord(s.min(WING_TIP));
        let x = (p.z + 0.25 * c) / c;
        let upper = n.y > 0.0;
        if t.part == super::surfaces::PART_COVE {
            paint.color = Vec3::splat(0.12);
            paint.rough = 0.6;
            paint.ao = 0.35;
            return paint;
        }
        // Tip livery.
        let tipk = band(s, 4.20, 9.0, size);
        paint.color = mix(paint.color, navy, tipk);
        let pin = band(s, 4.10, 4.135, size);
        paint.color = mix(paint.color, orange, pin);
        // Walkway at the root, upper surface behind the spar.
        if upper && s < 0.45 && x > 0.36 {
            let k = band(s, -1.0, 0.40, size) * band(x, 0.38, 2.0, size / c);
            let grain = noise2(Vec2::new(p.x * 900.0, p.z * 900.0), 21);
            paint.color = mix(paint.color, Vec3::splat(0.075 + 0.03 * grain), k);
            paint.rough = paint.rough + (0.88 - paint.rough) * k;
            paint.height += 0.0002 * grain * k;
            // Thin white border line.
            let edge = line(s - 0.40, 0.004, size)
                .max(line((x - 0.38) * c, 0.004, size) * (s < 0.40) as u8 as f32);
            paint.color = mix(paint.color, white, edge * 0.0);
        }
        // Fuel filler cap.
        if upper {
            let q = Vec2::new(s - 1.35, (x - 0.30) * c);
            let d = q.length();
            if d < 0.06 {
                let ring = band(d, 0.040, 0.050, size);
                let cap = band(d, -1.0, 0.040, size);
                let k = ring.max(cap);
                paint.color = mix(
                    paint.color,
                    Vec3::splat(if ring > cap { 0.75 } else { 0.55 }),
                    k,
                );
                paint.metal = k;
                paint.rough = paint.rough + (0.3 - paint.rough) * k;
                gap(&mut paint, d - 0.05, 0.0007, size, 0.0007);
                let tab = rounded_rect(q, Vec2::new(0.022, 0.006), 0.004);
                gap(&mut paint, tab, 0.0006, size, 0.0005);
            }
        }
        // Inspection plates and a fuel drain under the wing.
        if !upper {
            for sc in [0.95f32, 2.05, 3.25] {
                let q = Vec2::new(s - sc, (x - 0.45) * c);
                let d = q.length();
                if d < 0.08 {
                    gap(&mut paint, d - 0.065, 0.0007, size, 0.0008);
                    let a = q.y.atan2(q.x);
                    let k =
                        (a / (std::f32::consts::TAU / 6.0)).round() * (std::f32::consts::TAU / 6.0);
                    let sp = Vec2::new(k.cos(), k.sin()) * 0.054;
                    fastener(&mut paint, q - sp, 0.003, size);
                }
            }
            let dq = Vec2::new(s - 0.55, (x - 0.12) * c).length();
            if dq < 0.012 {
                paint.color = Vec3::splat(0.6);
                paint.metal = 1.0;
                paint.rough = 0.3;
            }
        }
        // Landing and taxi lights behind a clear cover in the leading edge: polished
        // reflector bowls with the lamps at their centers.
        if (1.50..1.90).contains(&s) && x < 0.05 {
            let yl = p.y;
            let window = rounded_rect(Vec2::new(s - 1.70, yl), Vec2::new(0.165, 0.040), 0.03);
            if window < size {
                let inside = (-window / size + 0.5).clamp(0.0, 1.0);
                let mut c2 = Vec3::splat(0.32);
                let mut metal = 0.6;
                let mut rough = 0.25;
                for sc in [1.615f32, 1.785] {
                    let d = Vec2::new(s - sc, yl).length();
                    if d < 0.036 {
                        let dish = 1.0 - d / 0.036;
                        c2 = Vec3::splat(0.80 + 0.15 * dish);
                        metal = 1.0;
                        rough = 0.08;
                        let ring = line(d - 0.036, 0.0015, size);
                        c2 *= 1.0 - 0.5 * ring;
                        if d < 0.008 {
                            c2 = Vec3::new(1.0, 0.97, 0.88);
                            metal = 0.0;
                            rough = 0.2;
                        }
                    }
                }
                paint.color = mix(paint.color, c2, inside);
                paint.metal = metal * inside;
                paint.rough = paint.rough + (rough - paint.rough) * inside;
                gap(&mut paint, window, 0.0008, size, 0.0006);
            }
        }
        // Tip joint.
        gap(&mut paint, s - WING_TIP, 0.0007, size, 0.0008);
        // Riveted control surfaces: ribs, front spar and trailing edge.
        if t.part == super::surfaces::PART_CONTROL {
            let (a, _) = if s < FLAP_SPAN.1 + 0.02 {
                FLAP_SPAN
            } else {
                AILERON_SPAN
            };
            let pitch = 0.205;
            let rel = s - a - 0.10;
            let rib_d = rel - (rel / pitch).round() * pitch;
            if x > 0.775 && x < 0.985 {
                rivets(&mut paint, p.z, rib_d, 0.028, 0.0021, size);
            }
            for xr in [0.795f32, 0.968] {
                rivets(&mut paint, s, (x - xr) * c, 0.032, 0.0021, size);
            }
        }
        // Leading edge: a little erosion and the odd insect.
        if x < 0.03 {
            let k = 1.0 - x / 0.03;
            paint.rough += 0.12 * k;
            let cell = Vec2::new(s * 160.0, p.y * 160.0);
            let r = rand(cell.x as i32, cell.y as i32, 0, 77);
            if r > 0.996 {
                paint.color *= 0.75;
            }
        }
        paint.rough += (noise2(Vec2::new(s * 2.0, x * 6.0), 9) - 0.5) * 0.04;
        paint
    })
}

/// Tail atlas parts: tailplane parts keep the surface ids, fin parts add 10.
pub const FIN_PART_OFFSET: u8 = 10;

/// The tail atlas: tailplanes, elevators with trim tabs, fin with the livery sweep, rudder.
pub fn paint_tail(atlas: &Atlas) -> Baked {
    let (white, navy, orange) = (hex(WHITE), hex(NAVY), hex(ORANGE));
    bake(atlas, false, 1.0, |t| {
        let p = t.p;
        let size = t.size;
        let mut paint = Paint::new(white, 0.17);
        let fin = t.part > FIN_PART_OFFSET;
        let part = if fin {
            t.part - FIN_PART_OFFSET
        } else {
            t.part
        };
        if part == super::surfaces::PART_COVE {
            paint.color = Vec3::splat(0.12);
            paint.rough = 0.6;
            paint.ao = 0.35;
            return paint;
        }
        if fin {
            // Fin frame: x = height (body y), z = body z.
            let y = p.x;
            let z = p.z;
            let line_y = 0.40 + (z - 3.55) * 0.62;
            let k = band(y, -1.0, line_y, size);
            paint.color = mix(paint.color, navy, k);
            let pin = band(y, line_y + 0.03, line_y + 0.052, size);
            paint.color = mix(paint.color, orange, pin);
            let cap = band(y, 1.66, 9.0, size);
            paint.color = mix(paint.color, navy, cap);
            if part == super::surfaces::PART_CONTROL {
                let pitch = 0.18;
                let rel = y - RUDDER_SPAN.0;
                let rib_d = rel - (rel / pitch).round() * pitch;
                if z > RUDDER_HINGE + 0.03 && z < fin_te_at(y) - 0.01 {
                    rivets(&mut paint, z, rib_d, 0.026, 0.002, size);
                }
                rivets(&mut paint, y, z - RUDDER_HINGE - 0.035, 0.03, 0.002, size);
            }
            return paint;
        }
        // Tailplane frame: x = span, z = body z.
        let s = p.x;
        let tipk = band(s, 1.62, 9.0, size);
        paint.color = mix(paint.color, navy, tipk);
        let pin = band(s, 1.55, 1.575, size);
        paint.color = mix(paint.color, orange, pin);
        if part == super::surfaces::PART_CONTROL {
            let pitch = 0.17;
            let rel = s - ELEVATOR_SPAN.0;
            let rib_d = rel - (rel / pitch).round() * pitch;
            rivets(&mut paint, p.z, rib_d, 0.026, 0.002, size);
            rivets(
                &mut paint,
                s,
                p.z - ELEVATOR_HINGE - 0.035,
                0.03,
                0.002,
                size,
            );
            // Trim tab outline and its hinge rivets.
            let te = tail_te_at(s);
            let tab_z = te - 0.085;
            if (0.55..1.0).contains(&s) {
                gap(&mut paint, p.z - tab_z, 0.0007, size, 0.0008);
                rivets(&mut paint, s, p.z - tab_z + 0.01, 0.02, 0.0018, size);
            }
            if p.z > tab_z {
                gap(&mut paint, s - 0.55, 0.0007, size, 0.0008);
                gap(&mut paint, s - 1.0, 0.0007, size, 0.0008);
            }
        }
        paint
    })
}

fn fin_te_at(y: f32) -> f32 {
    let p = fin_panel();
    (p.te)(y)
}

fn tail_te_at(s: f32) -> f32 {
    let p = tail_panel();
    (p.te)(s)
}

/// Tire textures (u around the wheel, v around the section): tread grooves, sidewall
/// lettering. Returns color, normal and orm images.
pub fn tire_textures() -> (Image, Image, Image) {
    let (w, h) = (1024usize, 256usize);
    let text = StrokeText::new("TUBELESS   6.00-6   8 PLY RATING   AVIATION", 5.6);
    let text_w = text.width();
    // Height in meters as a function of uv.
    let height = |u: f32, v: f32| -> f32 {
        let mut hgt = 0.0;
        // Tread: three grooves around the crown (v = 0.5).
        let dv = (v - 0.5).abs();
        if dv < 0.12 {
            for g in [0.0f32, 0.055] {
                let d = (dv - g).abs();
                hgt -= 0.0018 * (1.0 - (d / 0.008).min(1.0)).powf(0.5);
            }
        }
        // Sidewall lettering at v ≈ 0.27 and 0.73, twice around.
        for (vc, flip) in [(0.27f32, false), (0.73, true)] {
            let local_v = (v - vc) / 0.045;
            if local_v.abs() < 0.8 {
                let along = (u * 2.0).fract() * (text_w + 20.0);
                let gy = if flip {
                    3.0 - local_v * 3.0
                } else {
                    3.0 + local_v * 3.0
                };
                let d = text.distance(Vec2::new(along, gy));
                hgt += 0.0004 * (1.0 - ((d - 0.5) / 0.4).clamp(0.0, 1.0));
            }
        }
        hgt
    };
    // Heights once, then the three images from the buffer.
    let mut hbuf = vec![0.0f32; w * h];
    par_rows(&mut hbuf, w, |y, row| {
        for (x, v) in row.iter_mut().enumerate() {
            *v = height((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
        }
    });
    let at = |u: f32, v: f32| -> f32 {
        let x = ((u * w as f32) as isize).rem_euclid(w as isize) as usize;
        let y = ((v * h as f32) as isize).clamp(0, h as isize - 1) as usize;
        hbuf[y * w + x]
    };
    let normal = normal_image(w, h, 1.4, 0.30, at);
    let color = image_fn(w, h, true, |u, v| {
        let wear = (1.0 - ((v - 0.5).abs() / 0.12).min(1.0)) * 0.012;
        let n = noise2(Vec2::new(u * 300.0, v * 60.0), 4) * 0.006;
        let letters = (at(u, v) > 0.0001) as u8 as f32 * 0.01;
        let c = 0.022 + wear + n + letters;
        let s = srgb(Vec3::new(c, c * 0.98, c * 0.96));
        [s[0], s[1], s[2], 255]
    });
    let orm = image_fn(w, h, false, |u, v| {
        let tread = 1.0 - ((v - 0.5).abs() / 0.13).min(1.0);
        let r = 0.86 - 0.12 * tread - 0.08 * noise2(Vec2::new(u * 200.0, v * 40.0), 8);
        let groove = (at(u, v) < -0.0005) as u8 as f32;
        let ao = 1.0 - 0.5 * groove;
        [(ao * 255.0) as u8, (r * 255.0) as u8, 0, 255]
    });
    (color, normal, orm)
}

/// Tiling seat leather: horizontal pleats with stitching (normal map).
pub fn leather_normal() -> Image {
    normal_image(256, 256, 0.25, 0.25, |u, v| {
        let pleat = ((v * 6.0).fract() - 0.5).abs() * 2.0;
        let roll = -0.0025 * pleat.powf(3.0);
        let stitch_line = ((v * 6.0).fract()).min(1.0 - (v * 6.0).fract());
        let stitch = if stitch_line < 0.04 && (u * 40.0).fract() < 0.55 {
            -0.0004
        } else {
            0.0
        };
        let grain = (noise2(Vec2::new(u * 160.0, v * 160.0), 2) - 0.5) * 0.00012;
        roll + stitch + grain
    })
}

/// The cockpit displays: primary flight display (left half), moving map (right half),
/// standby gauges and the radio along the bottom strip.
pub fn screen_image() -> Image {
    let (w, h) = (1024usize, 512usize);
    let speeds = [
        (StrokeText::new("130", 5.4), 0.25f32),
        (StrokeText::new("120", 5.4), 0.40),
        (StrokeText::new("100", 5.4), 0.70),
        (StrokeText::new("90", 5.4), 0.85),
    ];
    let alts = [
        (StrokeText::new("1700", 5.4), 0.25f32),
        (StrokeText::new("1600", 5.4), 0.40),
        (StrokeText::new("1400", 5.4), 0.70),
        (StrokeText::new("1300", 5.4), 0.85),
    ];
    let speed_now = StrokeText::new("110", 5.4);
    let alt_now = StrokeText::new("1520", 5.4);
    let heads = [
        (StrokeText::new("33", 5.4), 0.25f32),
        (StrokeText::new("N", 5.4), 0.50),
        (StrokeText::new("3", 5.4), 0.75),
    ];
    let mfd_top = StrokeText::new("MAP  NRST  FPL", 5.4);
    let eng = [
        StrokeText::new("RPM 2450", 5.4),
        StrokeText::new("MAP 24.8", 5.4),
        StrokeText::new("OIL 82", 5.4),
        StrokeText::new("FUEL 31", 5.4),
    ];
    let com = StrokeText::new("124.850", 5.4);
    let com2 = StrokeText::new("121.500", 5.4);
    let text_px = |t: &StrokeText, q: Vec2, origin: Vec2, scale: f32| -> f32 {
        let g = (q - origin) / scale;
        let d = t.distance(Vec2::new(g.x, -g.y + 6.0)) * scale;
        (1.0 - (d - scale * 0.45) / (scale * 0.35)).clamp(0.0, 1.0)
    };
    image_fn(w, h, true, |u, v| {
        let mut c;
        if v < 0.75 {
            if u < 0.5 {
                // ---- PFD
                let a = u / 0.5;
                let b = v / 0.75;
                let horizon = 0.53 + (a - 0.5) * 0.06;
                c = if b < horizon {
                    let t = b / horizon;
                    Vec3::new(0.05, 0.22, 0.75).lerp(Vec3::new(0.30, 0.55, 0.95), t)
                } else {
                    let t = ((b - horizon) / (1.0 - horizon)).min(1.0);
                    Vec3::new(0.42, 0.25, 0.10).lerp(Vec3::new(0.25, 0.14, 0.05), t)
                };
                let white = Vec3::splat(0.95);
                let q = Vec2::new(a, b);
                // Horizon line and pitch ladder.
                let hl = line(b - horizon, 0.003, 0.002);
                c = c.lerp(white, hl);
                for k in [-3i32, -2, -1, 1, 2, 3] {
                    let y = horizon + k as f32 * 0.07;
                    let half = if k.abs() % 2 == 0 { 0.09 } else { 0.05 };
                    if (a - 0.5).abs() < half {
                        c = c.lerp(white, line(b - y, 0.0025, 0.002));
                    }
                }
                // Aircraft symbol.
                let yellow = Vec3::new(1.0, 0.75, 0.05);
                let wing_l = (b - horizon - 0.0).abs() < 0.008 && (0.30..0.42).contains(&a);
                let wing_r = (b - horizon).abs() < 0.008 && (0.58..0.70).contains(&a);
                let dot = Vec2::new((a - 0.5) * 2.0, b - horizon).length() < 0.012;
                if wing_l || wing_r || dot {
                    c = yellow;
                }
                // Speed and altitude tapes.
                for (x0, x1) in [(0.03f32, 0.18f32), (0.80, 0.97)] {
                    if (x0..x1).contains(&a) && (0.12..0.95).contains(&b) {
                        c = c * 0.25 + Vec3::splat(0.03);
                    }
                }
                for (t, y) in &speeds {
                    c = c.lerp(white, text_px(t, q, Vec2::new(0.05, *y - 0.025), 0.009));
                }
                for (t, y) in &alts {
                    c = c.lerp(white, text_px(t, q, Vec2::new(0.815, *y - 0.025), 0.007));
                }
                for (x0, x1, t, s) in [
                    (0.02f32, 0.19f32, &speed_now, 0.010f32),
                    (0.79, 0.98, &alt_now, 0.008),
                ] {
                    if (x0..x1).contains(&a) && (0.50..0.60).contains(&b) {
                        c = Vec3::splat(0.0);
                        let edge = (a - x0).min(x1 - a).min(b - 0.5).min(0.6 - b);
                        if edge < 0.004 {
                            c = white;
                        }
                        c = c.lerp(white, text_px(t, q, Vec2::new(x0 + 0.02, 0.523), s));
                    }
                }
                // Heading strip.
                if b > 0.93 {
                    c = Vec3::splat(0.04);
                    let tick = (a * 24.0).fract() < 0.06;
                    if tick && b > 0.975 {
                        c = white;
                    }
                }
                for (t, x) in &heads {
                    c = c.lerp(white, text_px(t, q, Vec2::new(*x - 0.02, 0.935), 0.005));
                }
            } else {
                // ---- moving map
                let a = (u - 0.5) / 0.5;
                let b = v / 0.75;
                let q = Vec2::new(a, b);
                let m = fbm3(Vec3::new(a * 5.0, b * 3.5, 0.3), 31, 4);
                c = if m < 0.36 {
                    Vec3::new(0.03, 0.10, 0.32)
                } else if m < 0.55 {
                    Vec3::new(0.10, 0.22, 0.08).lerp(Vec3::new(0.20, 0.30, 0.10), (m - 0.36) / 0.19)
                } else {
                    Vec3::new(0.30, 0.24, 0.12)
                        .lerp(Vec3::new(0.45, 0.38, 0.25), ((m - 0.55) / 0.2).min(1.0))
                };
                // Route, range ring, ownship.
                let route = segment_distance(q, Vec2::new(0.56, 0.62), Vec2::new(0.78, 0.18)).min(
                    segment_distance(q, Vec2::new(0.78, 0.18), Vec2::new(0.95, 0.10)),
                );
                c = c.lerp(Vec3::new(0.9, 0.1, 0.85), line(route, 0.004, 0.003));
                let ring = (Vec2::new((a - 0.6) * 1.3, b - 0.6).length() - 0.3).abs();
                c = c.lerp(Vec3::splat(0.85), line(ring, 0.0015, 0.002) * 0.7);
                let own = Vec2::new((a - 0.6) * 1.3, b - 0.62);
                if own.y.abs() < 0.035
                    && own.x.abs() < (0.035 - own.y.abs() * 0.0) * (0.5 + own.y / 0.07)
                {
                    c = Vec3::splat(0.95);
                }
                // Engine strip and top bar.
                if a < 0.22 {
                    c = Vec3::splat(0.03);
                    for (k, t) in eng.iter().enumerate() {
                        c = c.lerp(
                            Vec3::new(0.3, 0.95, 0.4),
                            text_px(t, q, Vec2::new(0.015, 0.12 + 0.2 * k as f32), 0.0045),
                        );
                    }
                    // RPM bar.
                    if (0.02..0.20).contains(&a) && (0.07..0.09).contains(&b) {
                        c = if a < 0.16 {
                            Vec3::new(0.1, 0.8, 0.2)
                        } else {
                            Vec3::splat(0.2)
                        };
                    }
                }
                if b < 0.07 {
                    c = Vec3::splat(0.05);
                    c = c.lerp(
                        Vec3::new(0.2, 0.85, 0.95),
                        text_px(&mfd_top, q, Vec2::new(0.3, 0.012), 0.0065),
                    );
                }
            }
        } else if u < 0.375 {
            // ---- standby gauges
            let k = (u / 0.125) as usize;
            let a = (u - k as f32 * 0.125) / 0.125 * 2.0 - 1.0;
            let b = (v - 0.75) / 0.25 * 2.0 - 1.0;
            let r = Vec2::new(a, b).length();
            c = Vec3::splat(0.015);
            if k == 2 && r < 0.8 {
                c = if b < 0.05 {
                    Vec3::new(0.06, 0.2, 0.55)
                } else {
                    Vec3::new(0.35, 0.2, 0.08)
                };
                if (b - 0.05).abs() < 0.03 {
                    c = Vec3::splat(0.9);
                }
            }
            let ang = b.atan2(a);
            let ticks =
                ((ang / std::f32::consts::TAU * 36.0).fract() < 0.12) && (0.72..0.85).contains(&r);
            if ticks {
                c = Vec3::splat(0.9);
            }
            if k == 0 && (0.78..0.84).contains(&r) && ang > -0.5 && ang < 2.0 {
                c = Vec3::new(0.1, 0.8, 0.2);
            }
            let needle_a: f32 = if k == 0 { 0.9 } else { -1.2 };
            let dir = Vec2::new(needle_a.cos(), needle_a.sin());
            let along = Vec2::new(a, b).dot(dir);
            let across = Vec2::new(a, b).perp_dot(dir).abs();
            if k != 2 && across < 0.04 && (-0.1..0.7).contains(&along) {
                c = Vec3::splat(0.95);
            }
            if r > 0.95 {
                c = Vec3::splat(0.0);
            }
        } else if (0.5..0.75).contains(&u) {
            // ---- radio
            let q = Vec2::new((u - 0.5) / 0.25, (v - 0.75) / 0.25);
            c = Vec3::splat(0.01);
            c = c.lerp(
                Vec3::new(0.2, 0.95, 0.35),
                text_px(&com, q, Vec2::new(0.1, 0.2), 0.035),
            );
            c = c.lerp(
                Vec3::new(0.95, 0.6, 0.1),
                text_px(&com2, q, Vec2::new(0.1, 0.6), 0.035),
            );
        } else {
            c = Vec3::splat(0.0);
        }
        let s = srgb(c);
        [s[0], s[1], s[2], 255]
    })
}
