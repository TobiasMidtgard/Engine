//! The pictures a pact is painted with, made in code once (`demons::DemonMeshes`): a hellgate's
//! vortex (a swirl of hellfire, the bright arms turning over it and the black eye at its heart)
//! and the glowing cracks of hellstone (the gates' stones, the hounds, the meteors' rock).
//!
//! All of them are emission: the colours run from black through blood red and crimson to an
//! orange-gold, so the material's emissive strength sets how hot they glow.

use std::f32::consts::TAU;

use engine_core::math::{Vec2, Vec3};
use engine_render::{Image, Mesh, Vertex};

/// A vortex picture's size, px.
pub const VORTEX_PX: u32 = 256;
/// The cracks picture's size, px, and its cells (large cracks) across.
pub const CRACKS_PX: u32 = 512;
pub const CRACK_CELLS: i32 = 6;

pub(crate) fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub(crate) fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0x00ff_ffff) as f32 / 16_777_215.0
}

/// Smooth value noise in 0..1, repeating every `period` cells.
pub(crate) fn noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let h = |dx: i32, dy: i32| {
        hash2(
            (xi + dx).rem_euclid(period),
            (yi + dy).rem_euclid(period),
            seed,
        )
    };
    lerp(lerp(h(0, 0), h(1, 0), ux), lerp(h(0, 1), h(1, 1), ux), uy)
}

/// Four octaves of `noise`, 0..1, repeating every `period` cells of the first.
pub(crate) fn fbm(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut f, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for o in 0..4 {
        sum += amp * noise(x * f, y * f, period * f as i32, seed.wrapping_add(o * 977));
        norm += amp;
        amp *= 0.5;
        f *= 2.0;
    }
    sum / norm
}

/// Hellfire by heat, linear RGB: black, blood red, crimson, orange, gold.
pub fn hellfire(t: f32) -> [f32; 3] {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [0.0, 0.0, 0.0]),
        (0.3, [0.16, 0.005, 0.002]),
        (0.6, [0.62, 0.04, 0.01]),
        (0.85, [1.0, 0.2, 0.025]),
        (1.0, [1.0, 0.52, 0.12]),
    ];
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (w[0], w[1]);
        if t <= b {
            let u = (t - a) / (b - a);
            return [
                lerp(ca[0], cb[0], u),
                lerp(ca[1], cb[1], u),
                lerp(ca[2], cb[2], u),
            ];
        }
    }
    STOPS[4].1
}

pub(crate) fn srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

pub(crate) fn pixel(c: [f32; 3], alpha: f32) -> [u8; 4] {
    [
        srgb(c[0]),
        srgb(c[1]),
        srgb(c[2]),
        (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
    ]
}

/// The layers of a gate's vortex, back to front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VortexLayer {
    /// A whole disc of hellfire wound into five arms, dark at its heart.
    Swirl,
    /// Three bright arms (cut out: the swirl shows between them).
    Arms,
    /// The black eye at the heart, its ragged rim glowing.
    Eye,
}

impl VortexLayer {
    pub const ALL: [VortexLayer; 3] = [VortexLayer::Swirl, VortexLayer::Arms, VortexLayer::Eye];

    /// The heat at a point of the unit disc (x right, y up) and how much of it is there
    /// (the cut-out's alpha).
    pub fn at(self, x: f32, y: f32) -> (f32, f32) {
        let r = (x * x + y * y).sqrt().min(1.0);
        let theta = y.atan2(x);
        let ln_r = r.max(0.02).ln();
        match self {
            VortexLayer::Swirl => {
                let warp = fbm(x * 3.0 + 7.0, y * 3.0 + 7.0, 4096, 11) - 0.5;
                let phi = 5.0 * (theta - 2.4 * ln_r) + 3.2 * warp;
                let a = 0.5 + 0.5 * phi.cos();
                let mottle = fbm(x * 7.0 + 3.0, y * 7.0 + 3.0, 4096, 23);
                let glow = smoothstep(0.1, 0.42, r) * (1.0 - 0.45 * smoothstep(0.75, 1.0, r));
                let t = glow * (0.12 + 0.88 * a * a) * (0.8 + 0.4 * mottle);
                let rim = 0.65 * smoothstep(0.86, 1.0, r) * (0.6 + 0.4 * a);
                (t.max(rim), 1.0)
            }
            VortexLayer::Arms => {
                let warp = fbm(x * 2.5 + 1.0, y * 2.5 + 1.0, 4096, 37) - 0.5;
                let phi = 3.0 * (theta - 2.0 * ln_r) + 2.4 * warp;
                let a = 0.5 + 0.5 * phi.cos();
                // Thin near the eye, broad mid-way, fraying out before the rim.
                let thr = 0.62 + 0.3 * (1.0 - smoothstep(0.15, 0.5, r));
                let fade = smoothstep(0.12, 0.25, r) * (1.0 - smoothstep(0.86, 0.97, r));
                let cover = smoothstep(thr - 0.05, thr + 0.05, a) * fade;
                let hot = ((a - thr) / (1.0 - thr)).clamp(0.0, 1.0);
                let mottle = fbm(x * 9.0, y * 9.0, 4096, 41);
                let t = 0.62
                    + 0.38 * hot.powf(0.7) * (0.8 + 0.4 * mottle)
                    + 0.1 * (1.0 - smoothstep(0.3, 0.9, r));
                (t, cover)
            }
            VortexLayer::Eye => {
                // A ragged edge: noise round the circle (repeating once round).
                let ragged = noise(theta / TAU * 12.0 + 12.0, 0.5, 12, 53) - 0.5;
                let edge = 0.82 + 0.16 * ragged;
                let cover = 1.0 - smoothstep(edge - 0.03, edge + 0.03, r);
                let g = smoothstep(edge - 0.24, edge, r);
                (0.95 * g * g, cover)
            }
        }
    }

    /// Its picture: heat as hellfire's colour, cover as alpha.
    pub fn image(self) -> Image {
        let n = VORTEX_PX;
        let mut img = Image::from_fn(n, n, true, |px, py| {
            let x = (px as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let y = 1.0 - (py as f32 + 0.5) / n as f32 * 2.0;
            let (t, cover) = self.at(x, y);
            pixel(hellfire(t), cover)
        });
        img.generate_mips();
        img
    }
}

/// Distance to the nearest and second-nearest jittered cell point (Worley), repeating every
/// `cells` cells.
fn worley(x: f32, y: f32, cells: i32, seed: u32) -> (f32, f32) {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (mut f1, mut f2) = (f32::MAX, f32::MAX);
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (xi + dx, yi + dy);
            let (wx, wy) = (cx.rem_euclid(cells), cy.rem_euclid(cells));
            let px = cx as f32 + 0.1 + 0.8 * hash2(wx, wy, seed);
            let py = cy as f32 + 0.1 + 0.8 * hash2(wx, wy, seed ^ 0x5555);
            let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2)
}

/// The heat of hellstone's cracks at a point of its tile (0..1 each way): wide cracks between
/// big plates, fine ones crazing the plates, both broken up so they come and go.
pub fn crack_heat(u: f32, v: f32) -> f32 {
    let c = CRACK_CELLS;
    let (x, y) = (u * c as f32, v * c as f32);
    let (f1, f2) = worley(x, y, c, 71);
    let wide = 1.0 - smoothstep(0.0, 0.11, f2 - f1);
    let (g1, g2) = worley(x * 2.5, y * 2.5, (c as f32 * 2.5) as i32, 83);
    let fine = 1.0 - smoothstep(0.0, 0.07, g2 - g1);
    let breaks = smoothstep(
        0.32,
        0.6,
        fbm(x * 1.5, y * 1.5, (c as f32 * 1.5) as i32, 97),
    );
    let flicker = 0.6 + 0.4 * fbm(x * 4.0, y * 4.0, c * 4, 101);
    (wide * (0.45 + 0.55 * breaks) + 0.45 * fine * breaks).min(1.0) * flicker
}

/// Hellstone's cracks: a tiling picture, black where the stone is whole.
pub fn crack_image() -> Image {
    let n = CRACKS_PX;
    let mut img = Image::from_fn(n, n, true, |px, py| {
        let h = crack_heat((px as f32 + 0.5) / n as f32, (py as f32 + 0.5) / n as f32);
        let c = hellfire(0.62 + 0.38 * h);
        pixel([c[0] * h, c[1] * h, c[2] * h], 1.0)
    });
    img.generate_mips();
    img
}

/// A round disc of radius 1 in the XY plane, seen from both sides (its front faces -Z), the
/// pictures laid flat across it.
pub fn disc_mesh() -> Mesh {
    let n = 64u32;
    let mut m = Mesh::new(Vec::new(), Vec::new());
    for normal in [Vec3::NEG_Z, Vec3::Z] {
        let base = m.vertices.len() as u32;
        m.vertices
            .push(Vertex::new(Vec3::ZERO, normal, Vec2::splat(0.5)));
        for i in 0..n {
            let (s, c) = (i as f32 / n as f32 * TAU).sin_cos();
            m.vertices.push(Vertex::new(
                Vec3::new(c, s, 0.0),
                normal,
                Vec2::new(0.5 + 0.5 * c, 0.5 - 0.5 * s),
            ));
        }
        for i in 0..n {
            let (a, b) = (base + 1 + i, base + 1 + (i + 1) % n);
            if normal.z < 0.0 {
                m.indices.extend([base, b, a]);
            } else {
                m.indices.extend([base, a, b]);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vortex_is_dark_at_its_heart_and_its_arms_leave_gaps() {
        // The swirl's heart is black, its body glows.
        assert!(VortexLayer::Swirl.at(0.0, 0.02).0 < 0.05);
        let ring: Vec<f32> = (0..64)
            .map(|i| {
                let (s, c) = (i as f32 / 64.0 * TAU).sin_cos();
                VortexLayer::Swirl.at(0.6 * c, 0.6 * s).0
            })
            .collect();
        let (lo, hi) = ring
            .iter()
            .fold((1.0f32, 0.0f32), |(l, h), &t| (l.min(t), h.max(t)));
        assert!(hi > 0.5 && lo < 0.35, "{lo} {hi}");
        // The arms cover some of a ring and leave the rest open.
        let cover = (0..128)
            .filter(|&i| {
                let (s, c) = (i as f32 / 128.0 * TAU).sin_cos();
                VortexLayer::Arms.at(0.55 * c, 0.55 * s).1 > 0.5
            })
            .count();
        assert!((15..90).contains(&cover), "{cover}");
        // The eye is black in the middle, hot at its rim and open outside.
        let (t, cover) = VortexLayer::Eye.at(0.0, 0.0);
        assert!(t < 0.01 && cover > 0.99);
        let rim = (60..95)
            .map(|i| VortexLayer::Eye.at(i as f32 / 100.0, 0.0))
            .filter(|&(_, cover)| cover > 0.5)
            .fold(0.0f32, |m, (t, _)| m.max(t));
        assert!(rim > 0.5, "{rim}");
        assert!(VortexLayer::Eye.at(0.99, 0.0).1 < 0.01);
    }

    #[test]
    fn cracks_tile_and_cover_a_little_of_the_stone() {
        // The edges meet: the heat at u = 0 and u = 1 agrees.
        for i in 0..32 {
            let v = i as f32 / 32.0;
            assert!((crack_heat(0.0, v) - crack_heat(1.0, v)).abs() < 0.02);
            assert!((crack_heat(v, 0.0) - crack_heat(v, 1.0)).abs() < 0.02);
        }
        // Mostly whole stone, some of it glowing.
        let n = 96;
        let hot = (0..n * n)
            .filter(|i| crack_heat((i % n) as f32 / n as f32, (i / n) as f32 / n as f32) > 0.3)
            .count() as f32
            / (n * n) as f32;
        assert!(hot > 0.04 && hot < 0.35, "{hot}");
    }

    #[test]
    fn hellfire_runs_from_black_to_gold() {
        assert_eq!(hellfire(0.0), [0.0; 3]);
        let mid = hellfire(0.6);
        assert!(mid[0] > 0.5 && mid[1] < 0.1);
        let top = hellfire(1.0);
        assert!(top[1] > 0.4);
    }

    #[test]
    fn the_disc_faces_both_ways() {
        let m = disc_mesh();
        for tri in m.indices.chunks(3) {
            let p = |i: u32| Vec3::from_array(m.vertices[i as usize].position);
            let n = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
            let want = Vec3::from_array(m.vertices[tri[0] as usize].normal);
            assert!(n.dot(want) > 0.0);
        }
    }
}
