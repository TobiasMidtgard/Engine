//! Car wheels: tires (lathed carcass with sidewall bulge, rounded shoulders, tread grooves;
//! gravel tires add chunky blocks), cast rims with dished spokes, brake discs and calipers.
//!
//! Wheel frame: the axle is X, the outer face of the wheel points +x (left wheels are the
//! same meshes turned half a turn about Y), the wheel center is the origin.

use engine_core::math::{Quat, Vec2, Vec3, Vec4};
use engine_render::{Mesh, Vertex};

use super::parts::{lathe_x, loft_rings, merge, orient_like_normals, transformed, trs};

/// Rim designs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RimStyle {
    /// Five pairs of slim, deeply dished spokes (two-tone, machined faces).
    TwinSpoke,
    /// Eight straight, wide rally spokes with a shallow dish.
    Rally,
}

/// Tread patterns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tread {
    /// Road performance tire: circumferential grooves, sipes in the normal map.
    Performance,
    /// Gravel rally tire: tall separate blocks.
    Gravel,
}

/// Wheel dimensions and styles, meters.
#[derive(Debug, Clone, Copy)]
pub struct WheelSpec {
    pub radius: f32,
    pub width: f32,
    /// Bead seat radius (half the rim diameter).
    pub rim: f32,
    pub style: RimStyle,
    pub tread: Tread,
    pub disc: f32,
}

impl WheelSpec {
    /// Rim half width (bead to bead).
    pub fn rim_half(&self) -> f32 {
        self.width * 0.47
    }
}

/// Tire carcass profile (axial x, radius), inner bead → over the tread → outer bead.
fn tire_profile(s: &WheelSpec) -> (Vec<Vec2>, Vec<usize>) {
    let r = s.radius;
    let hw = s.width * 0.5;
    let bead = s.rim + 0.011;
    let side_h = r - bead;
    let mut half: Vec<Vec2> = Vec::new();
    // Inner half from the bead up to the tread center (x negative).
    let bulge = hw * 1.035;
    let pts = [
        (hw * 0.93, bead),
        (hw * 0.985, bead + side_h * 0.16),
        (bulge, bead + side_h * 0.42),
        (hw * 1.02, bead + side_h * 0.63),
        (hw * 0.975, bead + side_h * 0.80),
        (hw * 0.90, r - 0.0075),
        (hw * 0.82, r - 0.0025),
        (hw * 0.70, r),
    ];
    for (x, rr) in pts {
        half.push(Vec2::new(-x, rr));
    }
    let mut sharp = Vec::new();
    // Tread with two circumferential grooves on each side of the center.
    let grooves: &[f32] = match s.tread {
        Tread::Performance => &[-0.52, -0.17],
        Tread::Gravel => &[],
    };
    let mut prof = half.clone();
    for &g in grooves {
        let x = g * hw;
        let gw = 0.0065;
        for (dx, dr, edge) in [
            (-gw, 0.0, true),
            (-gw * 0.7, -0.008, true),
            (gw * 0.7, -0.008, true),
            (gw, 0.0, true),
        ] {
            if edge {
                sharp.push(prof.len());
            }
            prof.push(Vec2::new(x + dx, r + dr));
        }
    }
    // Mirror to the outer half.
    let mut out = prof.clone();
    for p in prof.iter().rev() {
        let m = Vec2::new(-p.x, p.y);
        if (m - *out.last().unwrap()).length() > 1e-5 {
            out.push(m);
        }
    }
    let n = prof.len();
    let mirrored_sharp: Vec<usize> = sharp.iter().map(|&k| 2 * n - 1 - k - 1).collect();
    sharp.extend(mirrored_sharp);
    (out, sharp)
}

/// Where things sit in a tire's texture: profile length, the tread's v range and the outer
/// sidewall's radius → v mapping.
pub struct TireLayout {
    pub length: f32,
    pub tread_v: (f32, f32),
    outer: Vec<(f32, f32)>,
}

impl TireLayout {
    /// v of radius `r` on the outer sidewall.
    pub fn outer_v(&self, r: f32) -> f32 {
        for w in self.outer.windows(2) {
            let ((ra, va), (rb, vb)) = (w[0], w[1]);
            if (r <= ra && r >= rb) || (r >= ra && r <= rb) {
                let t = if (rb - ra).abs() < 1e-6 {
                    0.0
                } else {
                    (r - ra) / (rb - ra)
                };
                return va + (vb - va) * t;
            }
        }
        self.outer.last().map_or(1.0, |o| o.1)
    }
}

pub fn tire_layout(s: &WheelSpec) -> TireLayout {
    let (profile, _) = tire_profile(s);
    let mut acc = vec![0.0f32];
    for w in profile.windows(2) {
        acc.push(acc.last().unwrap() + (w[1] - w[0]).length());
    }
    let total = acc.last().copied().unwrap_or(1.0).max(1e-6);
    let on_tread: Vec<usize> = (0..profile.len())
        .filter(|&k| profile[k].y >= s.radius - 0.0105)
        .collect();
    let (first, last) = (on_tread[0], *on_tread.last().unwrap());
    TireLayout {
        length: total,
        tread_v: (acc[first] / total, acc[last] / total),
        outer: (last..profile.len())
            .map(|k| (profile[k].y, acc[k] / total))
            .collect(),
    }
}

/// The tire carcass (UV: u around, v across the profile).
pub fn tire(s: &WheelSpec) -> Mesh {
    let (profile, sharp) = tire_profile(s);
    flipped(lathe_x(&profile, 80, &sharp))
}

/// Tall tread blocks for gravel tires: `pitches` around, three rows across.
pub fn gravel_blocks(s: &WheelSpec, pitches: usize) -> Mesh {
    let r = s.radius;
    let hw = s.width * 0.5;
    let mut parts = Vec::new();
    let block_h = 0.016;
    for k in 0..pitches {
        let a0 = k as f32 / pitches as f32 * std::f32::consts::TAU;
        // Rows: shoulder blocks staggered with the center row.
        for (row, (xc, wx, phase, len)) in [
            (-0.70f32, 0.26f32, 0.0f32, 0.62f32),
            (0.0, 0.34, 0.5, 0.55),
            (0.70, 0.26, 0.0, 0.62),
        ]
        .into_iter()
        .enumerate()
        {
            let pitch = std::f32::consts::TAU / pitches as f32;
            let a = a0 + phase * pitch + if row == 1 { 0.0 } else { pitch * 0.08 };
            let along = pitch * r * len;
            let sz = Vec3::new(wx * hw, block_h * 2.0, along);
            // Block center sits on the carcass and stands block_h proud of it.
            let rr = r - 0.004 + block_h * 0.5;
            let rot = Quat::from_rotation_x(-a);
            let center = rot * Vec3::new(xc * hw, rr, 0.0);
            // Slight skew on the shoulder blocks, like a real gravel pattern.
            let skew = Quat::from_rotation_y(if row == 1 { 0.0 } else { 0.22 * xc.signum() });
            let b = block(sz, 0.0035);
            parts.push(transformed(b, trs(center, rot * skew, Vec3::ONE)));
        }
    }
    merge(parts)
}

/// A small chamfered block (for tread blocks): 8 cap verts + sides.
fn block(size: Vec3, chamfer: f32) -> Mesh {
    let h = size * 0.5;
    let c = chamfer;
    // Top face inset by the chamfer, bottom face full size (a slight taper).
    let top = [
        Vec3::new(-h.x + c, h.y, -h.z + c),
        Vec3::new(h.x - c, h.y, -h.z + c),
        Vec3::new(h.x - c, h.y, h.z - c),
        Vec3::new(-h.x + c, h.y, h.z - c),
    ];
    let bot = [
        Vec3::new(-h.x, -h.y, -h.z),
        Vec3::new(h.x, -h.y, -h.z),
        Vec3::new(h.x, -h.y, h.z),
        Vec3::new(-h.x, -h.y, h.z),
    ];
    let mut m = Mesh::default();
    let mut quad = |a: Vec3, b: Vec3, cc: Vec3, d: Vec3| {
        let n = (b - a).cross(cc - a).normalize_or(Vec3::Y);
        let base = m.vertices.len() as u32;
        for (p, uv) in [(a, Vec2::ZERO), (b, Vec2::X), (cc, Vec2::ONE), (d, Vec2::Y)] {
            m.vertices
                .push(Vertex::new(p, n, uv * 0.02 + Vec2::new(0.5, 0.02)));
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    };
    quad(top[0], top[3], top[2], top[1]);
    for i in 0..4 {
        let j = (i + 1) % 4;
        quad(bot[i], bot[j], top[j], top[i]);
    }
    m
}

/// Reverse a lathed part's normals (and its windings with them). Lathe normals point to the
/// right of the profile's direction of travel; parts traced the other way need this.
fn flipped(mut m: Mesh) -> Mesh {
    for v in &mut m.vertices {
        v.normal = (-Vec3::from(v.normal)).to_array();
    }
    orient_like_normals(&mut m);
    m
}

/// Spoke cross-section around a centerline point: `t` tangential unit, `a` axial unit (+x).
fn spoke_ring(c: Vec3, t: Vec3, a: Vec3, w_front: f32, w_back: f32, depth: f32) -> Vec<Vec3> {
    // Rounded trapezoid: front face (toward +x) wider than the back.
    let r = 0.004;
    let hf = w_front * 0.5;
    let hb = w_back * 0.5;
    let pts = [
        (hf - r, 0.0),
        (hf, -r),
        (hb + (hf - hb) * 0.4, -depth * 0.6),
        (hb, -depth),
        (-hb, -depth),
        (-(hb + (hf - hb) * 0.4), -depth * 0.6),
        (-hf, -r),
        (-hf + r, 0.0),
        (-hf * 0.4, 0.0015),
        (hf * 0.4, 0.0015),
    ];
    pts.iter().map(|&(x, y)| c + t * x + a * y).collect()
}

/// Rim face height (axial x of the spoke face) at radius `r`.
fn dish(s: &WheelSpec, r: f32) -> f32 {
    let hub_r = 0.075;
    let lip_x = s.rim_half() - 0.004;
    let k = ((r - hub_r) / (s.rim - 0.008 - hub_r)).clamp(0.0, 1.0);
    match s.style {
        RimStyle::TwinSpoke => lip_x - 0.055 + 0.055 * k.powf(1.7),
        RimStyle::Rally => lip_x - 0.03 + 0.03 * k.powf(1.3),
    }
}

/// The rim: barrel (visible inner surface and lip), spokes, hub with lug nuts.
/// Vertex colors mark machined spoke faces (bright) against painted sides (dark).
pub fn rim(s: &WheelSpec) -> Mesh {
    let half = s.rim_half();
    let r0 = s.rim;
    // Barrel inner surface, inner edge → outer lip (normals toward the axle), then the lip.
    let barrel_prof = vec![
        Vec2::new(-half, r0 + 0.012),
        Vec2::new(-half + 0.004, r0 - 0.004),
        Vec2::new(-half * 0.6, r0 - 0.028),
        Vec2::new(half * 0.45, r0 - 0.028),
        Vec2::new(half - 0.03, r0 - 0.016),
        Vec2::new(half - 0.012, r0 - 0.010),
        Vec2::new(half - 0.002, r0 - 0.002),
        Vec2::new(half + 0.003, r0 + 0.008),
        Vec2::new(half + 0.0, r0 + 0.016),
        Vec2::new(half - 0.006, r0 + 0.018),
    ];
    // Traced inner edge → lip, its normals already face the axle (and out on the lip).
    let mut barrel = lathe_x(&barrel_prof, 72, &[]);
    for v in &mut barrel.vertices {
        let lip = v.position[0] > half - 0.004;
        v.color = if lip {
            [0.9, 0.9, 0.9, 1.0]
        } else {
            [0.3, 0.3, 0.31, 1.0]
        };
    }
    orient_like_normals(&mut barrel);
    // Spokes.
    let mut spokes = Vec::new();
    let (count, pair, w_hub, w_rim, depth): (usize, f32, f32, f32, f32) = match s.style {
        RimStyle::TwinSpoke => (5, 1.0, 0.026, 0.030, 0.034),
        RimStyle::Rally => (8, 0.0, 0.040, 0.052, 0.022),
    };
    let hub_r = 0.072;
    let tip_r = r0 - 0.010;
    // Straight rally spokes need fewer stations than the curved twin spokes.
    let stations = if s.style == RimStyle::Rally { 6 } else { 12 };
    for k in 0..count {
        let base_a = k as f32 / count as f32 * std::f32::consts::TAU;
        let sides: &[f32] = if pair > 0.0 { &[-1.0, 1.0] } else { &[0.0] };
        for &side in sides {
            let mut rings = Vec::new();
            for i in 0..=stations {
                let f = i as f32 / stations as f32;
                let r = hub_r + (tip_r - hub_r) * f;
                // Twin spokes diverge from the hub toward the rim.
                let spread = side * (0.07 + 0.10 * f.powf(1.2));
                let a = base_a + spread;
                let radial = Vec3::new(0.0, a.cos(), a.sin());
                let tang = Vec3::new(0.0, -a.sin(), a.cos());
                // Spokes flare where they meet the rim.
                let flare = 1.0 + 0.45 * ((f - 0.82) / 0.18).clamp(0.0, 1.0).powi(2);
                let w = (w_hub + (w_rim - w_hub) * f) * flare;
                let x = dish(s, r);
                let c = radial * r + Vec3::X * x;
                // The face tilts with the dish so it stays perpendicular-ish to the view.
                rings.push(spoke_ring(
                    c,
                    tang,
                    Vec3::X,
                    w,
                    w * 0.62,
                    depth * (1.0 - 0.3 * f),
                ));
            }
            let mut m = loft_rings(&rings, true);
            for v in &mut m.vertices {
                // Machined front faces (normals facing +x) bright, sides painted dark.
                let n = Vec3::from(v.normal);
                let bright = ((n.x - 0.55) / 0.35).clamp(0.0, 1.0);
                let c = 0.07 + 0.83 * bright;
                v.color = [c, c, c * 1.02, 1.0];
            }
            spokes.push(m);
        }
    }
    // Hub: a domed center with a recessed cap.
    let hx = dish(s, hub_r);
    let hub_prof = vec![
        Vec2::new(hx - 0.03, hub_r + 0.006),
        Vec2::new(hx - 0.002, hub_r + 0.004),
        Vec2::new(hx + 0.003, hub_r - 0.008),
        Vec2::new(hx + 0.006, 0.05),
        Vec2::new(hx + 0.004, 0.036),
        Vec2::new(hx - 0.004, 0.032),
        Vec2::new(hx - 0.004, 0.028),
        Vec2::new(hx + 0.004, 0.026),
        Vec2::new(hx + 0.008, 0.012),
        Vec2::new(hx + 0.009, 0.0005),
    ];
    let mut hub = flipped(lathe_x(&hub_prof, 48, &[]));
    for v in &mut hub.vertices {
        let r = Vec2::new(v.position[1], v.position[2]).length();
        let c = if r < 0.03 { 0.05 } else { 0.08 };
        v.color = [c, c, c, 1.0];
    }
    // Lug nuts.
    let mut nuts = Vec::new();
    for k in 0..5 {
        let a = (k as f32 + 0.5) / 5.0 * std::f32::consts::TAU;
        let c = Vec3::new(hx + 0.004, a.cos() * 0.052, a.sin() * 0.052);
        let mut nut = flipped(lathe_x(
            &[
                Vec2::new(-0.004, 0.0105),
                Vec2::new(0.008, 0.0105),
                Vec2::new(0.010, 0.008),
                Vec2::new(0.0105, 0.004),
                Vec2::new(0.011, 0.0005),
            ],
            6,
            &[1],
        ));
        for v in &mut nut.vertices {
            v.color = [0.75, 0.75, 0.77, 1.0];
        }
        nuts.push(transformed(nut, trs(c, Quat::IDENTITY, Vec3::ONE)));
    }
    merge([barrel, merge(spokes), hub, merge(nuts)])
}

/// Brake disc (friction ring + hat). UV v: 0..0.5 outer friction face (drilling texture),
/// higher v the edge and hat.
pub fn disc(s: &WheelSpec) -> Mesh {
    let ro = s.disc;
    let ri = s.disc * 0.62;
    let t = 0.032;
    let x0 = -0.036;
    let x1 = x0 + t;
    let prof = vec![
        Vec2::new(x1, ri),
        Vec2::new(x1, ro - 0.002),
        Vec2::new(x1 - 0.002, ro),
        Vec2::new(x0 + 0.002, ro),
        Vec2::new(x0, ro - 0.002),
        Vec2::new(x0, ri),
    ];
    let mut ring = lathe_x(&prof, 72, &[1, 3]);
    // UV v: outer face 0..0.45, edge 0.45..0.55, inner face 0.55..1.
    for v in &mut ring.vertices {
        let p = Vec3::from(v.position);
        let r = Vec2::new(p.y, p.z).length();
        let f = ((r - ri) / (ro - ri)).clamp(0.0, 1.0);
        v.uv[1] = if (p.x - x1).abs() < 1e-4 && r < ro - 0.0015 {
            0.45 * f
        } else if (p.x - x0).abs() < 1e-4 && r < ro - 0.0015 {
            0.55 + 0.45 * (1.0 - f)
        } else {
            0.45 + 0.1 * ((p.x - x0) / t).clamp(0.0, 1.0)
        };
        v.color = [1.0; 4];
    }
    orient_like_normals(&mut ring);
    // Hat: from the inner edge of the ring out to the hub face.
    let hat_prof = vec![
        Vec2::new(x1, ri + 0.002),
        Vec2::new(x1 + 0.012, ri - 0.004),
        Vec2::new(x1 + 0.03, ri - 0.010),
        Vec2::new(x1 + 0.03, 0.03),
    ];
    let mut hat = flipped(lathe_x(&hat_prof, 48, &[]));
    for v in &mut hat.vertices {
        v.uv = [0.5, 0.995];
        v.color = [0.35, 0.35, 0.36, 1.0];
    }
    merge([ring, hat])
}

/// A fixed brake caliper bridging the disc at the rear of the wheel, its outer face toward +x.
/// `pistons`: 4 or 6 (sets its length).
pub fn caliper(s: &WheelSpec, pistons: u32) -> Mesh {
    let span = if pistons >= 6 { 0.62 } else { 0.48 };
    let center = 0.22f32;
    let r_in = s.disc * 0.70;
    let r_out = s.disc + 0.016;
    let (x_in, x_out) = (-0.075, 0.026);
    let stations = 14;
    let mut rings = Vec::new();
    for i in 0..=stations {
        let f = i as f32 / stations as f32;
        let a = center + (f - 0.5) * span;
        let radial = Vec3::new(0.0, a.sin(), a.cos());
        // Rounded ends: shrink the section near both ends.
        let e = (1.0 - (2.0 * f - 1.0).powi(4)).max(0.05).sqrt();
        let rm = 0.5 * (r_in + r_out);
        let hr = 0.5 * (r_out - r_in) * (0.55 + 0.45 * e);
        let xm = 0.5 * (x_in + x_out);
        let hx = 0.5 * (x_out - x_in) * (0.7 + 0.3 * e);
        let mut ring = Vec::new();
        for j in 0..16 {
            let t = j as f32 / 16.0 * std::f32::consts::TAU;
            let (sn, cs) = t.sin_cos();
            // Superellipse section (boxy with soft edges).
            let px = cs.signum() * cs.abs().powf(0.35) * hx;
            let pr = sn.signum() * sn.abs().powf(0.35) * hr;
            ring.push(Vec3::X * (xm + px) + radial * (rm + pr));
        }
        rings.push(ring);
    }
    loft_rings(&rings, true)
}

/// A dust shield / hub carrier disk behind the brake (hides the void behind the spokes).
pub fn backing(s: &WheelSpec) -> Mesh {
    let prof = vec![
        Vec2::new(-0.072, 0.0),
        Vec2::new(-0.072, s.disc * 0.9),
        Vec2::new(-0.062, s.disc + 0.01),
    ];
    let mut m = lathe_x(&prof, 40, &[]);
    for v in &mut m.vertices {
        v.normal = [1.0, 0.0, 0.0];
    }
    orient_like_normals(&mut m);
    m
}

/// Linear RGBA helper.
pub fn rgba(r: f32, g: f32, b: f32) -> Vec4 {
    Vec4::new(r, g, b, 1.0)
}
