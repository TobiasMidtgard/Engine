//! The showcase arena's art: a sandstone amphitheater with a sand floor.
//!
//! Everything is generated: tiling PBR texture sets (sand with ripples, footprints and scuffs;
//! sandstone with bedding, tool marks and pits; weathered timber; forged iron; worn red drum
//! paint with hazard markings; embroidered banner cloth; glowing coal; woven mats; leather), an
//! unlit flame sheet, and meshes built from cut-stone blocks with rounded, chipped edges,
//! turned and fluted shapes, planks, straps, studs, cloth, rubble and crossed flame cards.
//! Repeated pieces share one mesh (32 stand segments, four gates, eight columns, …) and
//! everything is keyed in [`ArtAssets`]. What one world built is kept for the process, so a
//! second world (another test, a reloaded level) gets the arena in milliseconds.
//!
//! [`dress`] puts the art on an arena laid out by `demo::spawn_demo`: visuals go on (or under)
//! the gameplay entities, whose colliders never change. The braziers' fires and lights
//! flicker through [`flicker_fires`]. Without the renderer it falls back to a few primitive
//! shapes so headless tests stay fast.
//!
//! Frames: Y up, meters. The stands are built per 10° sector in a frame where +Z points from
//! the arena center through the sector's middle and Z is the distance from the center; blocks
//! are laid out in "straight" coordinates and then sheared ([`shear`]) so their joints run
//! radially and neighboring sectors meet exactly.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::sync::{Arc, Mutex, OnceLock};

use bevy_ecs::system::SystemState;
use engine_core::math::hash_u64;
use engine_core::prelude::*;
use engine_render::{
    AlphaMode, Image, MaterialRef, Mesh, MeshRef, MeshRenderer, PointLight, Shape,
    StandardMaterial, Vertex,
};
use engine_world::physics::{Collider, ColliderShape};

mod grounds;

use grounds::{Grounds, Prop};

use super::ArtAssets;
use super::mesh::{
    BoxOpts, NEG_Y, NEG_Z, POS_Z, SdfOpts, append, arc_points, bake_par, box_lid, deform,
    image_par, lathe, offset_uv, paint, par_map, rounded_box, sdf_mesh, steps, surface, tint,
    transformed, tube,
};
use super::noise::rand3;
use super::sdf;
use super::tex::{Texel, TextureSet, hex, mix, smooth};

// ============================================================================== randomness

/// A small deterministic random source for layouts.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(hash_u64(seed ^ 0x5eed_a7e4_a000_0001))
    }

    /// Uniform 0..1.
    pub fn f(&mut self) -> f32 {
        self.0 = hash_u64(self.0.wrapping_add(0x9e37_79b9_7f4a_7c15));
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform `a..b`.
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }

    /// Uniform −1..1.
    pub fn signed(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
}

/// Split `a..b` into pieces between `min` and `max` long (the last one absorbs the rest).
fn run(rng: &mut Rng, a: f32, b: f32, min: f32, max: f32) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut x = a;
    while b - x > 1e-4 {
        let left = b - x;
        let mut len = rng.range(min, max);
        if left - len < min * 0.7 {
            len = if left <= max * 1.15 { left } else { left * 0.5 };
        }
        out.push((x, (x + len).min(b)));
        x += len;
    }
    out
}

// ============================================================================== noise

const DIRS: [Vec2; 16] = {
    let mut d = [Vec2::ZERO; 16];
    let mut i = 0;
    while i < 16 {
        // cos/sin of i * 22.5°, written out (const context).
        const H: f32 = std::f32::consts::FRAC_1_SQRT_2;
        let c = [
            1.0, 0.92388, H, 0.38268, 0.0, -0.38268, -H, -0.92388, -1.0, -0.92388, -H, -0.38268,
            0.0, 0.38268, H, 0.92388,
        ];
        d[i] = Vec2::new(c[i], c[(i + 12) % 16]);
        i += 1;
    }
    d
};

// The texture generators spend most of their time in these few helpers, so they are inlined
// even in unoptimized builds.

#[inline(always)]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// `i` wrapped into `0..p` (no wrapping for `p <= 0`); skips the division in the usual case
/// of being at most one period off.
#[inline(always)]
fn wrapi(i: i32, p: i32) -> i32 {
    if p <= 0 || (0..p).contains(&i) {
        i
    } else if (-p..0).contains(&i) {
        i + p
    } else if (p..2 * p).contains(&i) {
        i - p
    } else {
        i.rem_euclid(p)
    }
}

/// SplitMix64's finalizer (as `hash_u64`).
#[inline(always)]
fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[inline(always)]
fn hash2(x: i32, y: i32, seed: u32) -> u64 {
    mix64((x as u32 as u64) ^ ((y as u32 as u64) << 32) ^ ((seed as u64) << 17).rotate_left(23))
}

/// Gradient noise, about −0.7..0.7, tiling every `per` cells on each axis (0 = never).
#[inline(always)]
fn pn(p: Vec2, per: (i32, i32), seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (x0, y0) = (i.x as i32, i.y as i32);
    let (xa, xb) = (wrapi(x0, per.0), wrapi(x0 + 1, per.0));
    let (ya, yb) = (wrapi(y0, per.1), wrapi(y0 + 1, per.1));
    let g = |x: i32, y: i32, d: Vec2| DIRS[(hash2(x, y, seed) >> 60) as usize].dot(d);
    let (g00, g10) = (g(xa, ya, f), g(xb, ya, f - Vec2::X));
    let (g01, g11) = (g(xa, yb, f - Vec2::Y), g(xb, yb, f - Vec2::ONE));
    let (u, v) = (fade(f.x), fade(f.y));
    let a = g00 + (g10 - g00) * u;
    let b = g01 + (g11 - g01) * u;
    a + (b - a) * v
}

/// Fractal [`pn`], roughly −1..1; `p` is in cells of the first octave.
fn fb(p: Vec2, per: (i32, i32), octaves: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm, mut k) = (0.0, 1.0, 0.0, 1);
    for o in 0..octaves {
        sum += amp
            * pn(
                p * k as f32,
                (per.0 * k, per.1 * k),
                seed.wrapping_add(o * 101),
            );
        norm += amp;
        amp *= 0.5;
        k *= 2;
    }
    sum / norm * 1.4
}

/// Uniform 0..1 per integer cell.
#[inline(always)]
fn r2(x: i32, y: i32, seed: u32) -> f32 {
    (hash2(x, y, seed) >> 40) as f32 / (1u64 << 24) as f32
}

/// Nearest feature points of cellular noise.
#[derive(Debug, Clone, Copy)]
struct Cell {
    d1: f32,
    d2: f32,
    /// Random 0..1 per cell.
    id: f32,
    /// `p` minus the nearest feature point (cells).
    rel: Vec2,
}

#[inline(always)]
fn cell(p: Vec2, per: (i32, i32), seed: u32) -> Cell {
    let i = p.floor();
    let (x0, y0) = (i.x as i32, i.y as i32);
    let mut c = Cell {
        d1: f32::MAX,
        d2: f32::MAX,
        id: 0.0,
        rel: Vec2::ZERO,
    };
    const UNIT: f32 = 1.0 / (1u32 << 21) as f32;
    let mut best = 0u64;
    let xs = [-1, 0, 1].map(|d| wrapi(x0 + d, per.0));
    let ys = [-1, 0, 1].map(|d| wrapi(y0 + d, per.1));
    for dy in -1..=1 {
        for dx in -1..=1 {
            let (cx, cy) = (xs[(dx + 1) as usize], ys[(dy + 1) as usize]);
            // One hash per cell: 21 bits each for the point's x, y and the cell's id.
            let h = hash2(cx, cy, seed);
            let q = Vec2::new(
                (x0 + dx) as f32 + (h & 0x1f_ffff) as f32 * UNIT,
                (y0 + dy) as f32 + ((h >> 21) & 0x1f_ffff) as f32 * UNIT,
            );
            let d = q.distance(p);
            if d < c.d1 {
                c.d2 = c.d1;
                c.d1 = d;
                best = h;
                c.rel = p - q;
            } else if d < c.d2 {
                c.d2 = d;
            }
        }
    }
    c.id = ((best >> 42) & 0x1f_ffff) as f32 * UNIT;
    c
}

/// Shortest offset on a torus of size `size` (wrapping texture space).
fn wrap_delta(d: Vec2, size: f32) -> Vec2 {
    d - (d / size).round() * size
}

// ============================================================================== textures

/// Meters one repeat of each arena texture covers (set `uv_scale` to the inverse).
pub mod span {
    pub const SAND: f32 = 6.0;
    pub const RAKED: f32 = 4.0;
    pub const STONE: f32 = 2.0;
    pub const WOOD: f32 = 2.0;
    pub const IRON: f32 = 1.0;
    pub const LEATHER: f32 = 0.5;
    pub const MAT: f32 = 1.0;
    pub const COAL: f32 = 1.0;
}

/// A footprint or scuff in the sand texture (meters inside one 6 m tile).
#[derive(Debug, Clone, Copy)]
struct Mark {
    at: Vec2,
    dir: Vec2,
    left: bool,
    depth: f32,
    /// 0 = sandal print, 1 = drag scuff, 2 = knee or hand hollow.
    kind: u8,
    len: f32,
    width: f32,
}

struct Marks {
    marks: Vec<Mark>,
    grid: Vec<Vec<u16>>,
}

const MARK_CELLS: i32 = 12;

/// Footprints and scuffs in a `size` m tile: `tracks` walking tracks, `fights` spots of sand
/// churned by pivoting feet and `scuffs` loose drags.
fn marks(size: f32, seed: u64, tracks: u32, fights: u32, scuffs: u32) -> Marks {
    let mut rng = Rng::new(seed);
    let mut marks = Vec::new();
    // Walking tracks: alternating feet along gently curving paths.
    for _ in 0..tracks {
        let mut p = Vec2::new(rng.range(0.0, size), rng.range(0.0, size));
        let mut heading = rng.range(0.0, TAU);
        let turn = rng.signed() * 0.12;
        let stride = rng.range(0.62, 0.82);
        // Older tracks are shallower, half filled in.
        let depth = rng.range(0.002, 0.0065);
        for k in 0..rng.range(5.0, 11.0) as usize {
            let fwd = Vec2::new(heading.cos(), heading.sin());
            let side = Vec2::new(-fwd.y, fwd.x);
            let left = k % 2 == 0;
            let off = side * if left { 0.085 } else { -0.085 };
            let toe = if left { 0.12 } else { -0.12 } + rng.signed() * 0.08;
            let d = Vec2::from_angle(toe).rotate(fwd);
            marks.push(Mark {
                at: p + off,
                dir: d,
                left,
                depth: depth * rng.range(0.8, 1.2),
                kind: 0,
                len: 0.27,
                width: 0.1,
            });
            p += fwd * stride * 0.5;
            heading += turn + rng.signed() * 0.05;
        }
    }
    // Fighting spots: sand churned by pivoting feet, mostly smeared drags with a few prints.
    for _ in 0..fights {
        let c = Vec2::new(rng.range(0.0, size), rng.range(0.0, size));
        for _ in 0..rng.range(7.0, 13.0) as usize {
            let a = rng.range(0.0, TAU);
            let at = c + Vec2::from_angle(a) * rng.range(0.0, 0.9);
            let dir = Vec2::from_angle(rng.range(0.0, TAU));
            if rng.f() < 0.65 {
                marks.push(Mark {
                    at,
                    dir,
                    left: false,
                    depth: rng.range(0.002, 0.006),
                    kind: 1,
                    len: rng.range(0.2, 0.6),
                    width: rng.range(0.08, 0.16),
                });
            } else {
                marks.push(Mark {
                    at,
                    dir,
                    left: rng.f() < 0.5,
                    depth: rng.range(0.003, 0.008),
                    kind: 0,
                    len: 0.27,
                    width: 0.1,
                });
            }
        }
        if rng.f() < 0.5 {
            marks.push(Mark {
                at: c + Vec2::new(rng.signed(), rng.signed()) * 0.5,
                dir: Vec2::from_angle(rng.range(0.0, TAU)),
                left: false,
                depth: rng.range(0.01, 0.02),
                kind: 2,
                len: rng.range(0.2, 0.35),
                width: rng.range(0.14, 0.2),
            });
        }
    }
    // Loose scuffs and drags.
    for _ in 0..scuffs {
        marks.push(Mark {
            at: Vec2::new(rng.range(0.0, size), rng.range(0.0, size)),
            dir: Vec2::from_angle(rng.range(0.0, TAU)),
            left: false,
            depth: rng.range(0.0015, 0.0045),
            kind: 1,
            len: rng.range(0.25, 1.1),
            width: rng.range(0.03, 0.1),
        });
    }
    for m in &mut marks {
        m.at = Vec2::new(m.at.x.rem_euclid(size), m.at.y.rem_euclid(size));
    }
    let cs = size / MARK_CELLS as f32;
    let mut grid = vec![Vec::new(); (MARK_CELLS * MARK_CELLS) as usize];
    for (i, m) in marks.iter().enumerate() {
        let reach = m.len * 0.5 + m.width + 0.06;
        let (x0, x1) = (
            ((m.at.x - reach) / cs).floor() as i32,
            ((m.at.x + reach) / cs).floor() as i32,
        );
        let (y0, y1) = (
            ((m.at.y - reach) / cs).floor() as i32,
            ((m.at.y + reach) / cs).floor() as i32,
        );
        for y in y0..=y1 {
            for x in x0..=x1 {
                let k = (wrapi(y, MARK_CELLS) * MARK_CELLS + wrapi(x, MARK_CELLS)) as usize;
                grid[k].push(i as u16);
            }
        }
    }
    Marks { marks, grid }
}

/// Signed distance to an ellipse (approximate), meters.
fn ellipse(p: Vec2, c: Vec2, r: Vec2) -> f32 {
    let q = (p - c) / r;
    (q.length() - 1.0) * r.min_element()
}

/// Height change (meters) and how compacted the sand is (0..1) for one mark at `d` (meters
/// from its center, unrotated).
fn mark_effect(m: &Mark, d: Vec2) -> (f32, f32, f32) {
    let fwd = m.dir;
    let side = Vec2::new(-fwd.y, fwd.x);
    let mut q = Vec2::new(d.dot(side), d.dot(fwd));
    match m.kind {
        0 => {
            if m.left {
                q.x = -q.x;
            }
            // The sole: a tapered capsule from heel to toe, bent a little inward, with a
            // shallow waist. Loose sand blurs its edge.
            let (a, b) = (Vec2::new(0.0, -0.088), Vec2::new(0.014, 0.078));
            let ab = b - a;
            let t = ((q - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0);
            let r = 0.039 + 0.012 * t - 0.007 * (4.0 * t * (1.0 - t)).powi(2);
            let sole = (q - (a + ab * t)).length() - r;
            if sole > 0.06 {
                return (0.0, 0.0, 0.0);
            }
            // Deeper under the heel and the ball, sand pushed up around the edge, more of it
            // in front where the toes pushed off.
            let bell = |x: f32, c: f32, w: f32| (-((x - c) / w).powi(2)).exp();
            let press = 0.7 + 0.3 * bell(t, 0.1, 0.18).max(bell(t, 0.72, 0.15));
            let inside = smooth(0.014, -0.022, sole);
            let front = smooth(0.3, 1.0, t);
            let rim = bell(sole, 0.018, 0.016 + 0.01 * front);
            (
                -m.depth * press * inside + m.depth * (0.12 + 0.2 * front) * rim,
                inside,
                rim,
            )
        }
        1 => {
            let h = m.len * 0.5;
            let along = q.y.clamp(-h, h);
            let dist = Vec2::new(q.x, q.y - along).length() - m.width * 0.5;
            if dist > 0.03 {
                return (0.0, 0.0, 0.0);
            }
            let inside = smooth(0.01, -m.width * 0.4, dist);
            let taper = smooth(-h, -h + 0.15, q.y) * (0.6 + 0.4 * smooth(h, h - 0.1, q.y));
            let streaks = (q.x * TAU / 0.011 + (q.y * 3.0).sin()).sin() * 0.0003;
            let rim = (-((dist - 0.012) / 0.016).powi(2)).exp();
            (
                (-m.depth * inside + streaks * inside) * taper + m.depth * 0.3 * rim * taper,
                inside * taper,
                rim * taper,
            )
        }
        _ => {
            let e = ellipse(q, Vec2::ZERO, Vec2::new(m.width * 0.5, m.len * 0.5));
            if e > 0.04 {
                return (0.0, 0.0, 0.0);
            }
            let inside = smooth(0.015, -0.05, e);
            let rim = (-((e - 0.015) / 0.02).powi(2)).exp();
            (-m.depth * inside + m.depth * 0.25 * rim, inside, rim)
        }
    }
}

/// Trampled arena sand: overlapping tracks of sandal prints, fighting spots churned by
/// pivoting feet, drag scuffs, clods kicked loose, pebbles, lumpy ground mottled pale where
/// it lies loose and dry and darker where it was packed, and faint wind ripples (6 m
/// repeat).
pub fn sand_set() -> TextureSet {
    let marks = marks(span::SAND, 404, 34, 26, 70);
    let size = span::SAND;
    let cs = size / MARK_CELLS as f32;
    let px = 1024.0;
    bake_par(1024, size, |uv| {
        let m = uv * size;
        let low = fb(uv * 3.0, (3, 3), 4, 1);
        let mid = fb(uv * 20.0, (20, 20), 3, 2);
        let fine = fb(uv * 90.0, (90, 90), 2, 3);
        let (ix, iy) = ((uv.x * px) as i32, (uv.y * px) as i32);
        let speck = rand3(ix, iy, 0, 4);
        let speck2 = rand3(ix / 2, iy / 2, 1, 5);
        // Ripples across the wind, crests warped.
        let warp = fb(uv * 2.0, (2, 2), 3, 6) * 0.07 + fb(uv * 9.0, (9, 9), 2, 7) * 0.012;
        let t = ((uv.y + warp) * 52.0).rem_euclid(1.0);
        let ripple_shape = if t < 0.72 {
            smooth(0.0, 0.72, t)
        } else {
            1.0 - smooth(0.72, 1.0, t)
        };
        let ripple_amt = smooth(-0.15, 0.45, low) * (0.4 + 0.6 * smooth(-0.4, 0.3, mid));
        // Footprints and scuffs.
        let (cx, cy) = (
            (m.x / cs).floor() as i32 % MARK_CELLS,
            (m.y / cs).floor() as i32 % MARK_CELLS,
        );
        let (mut dig, mut raise, mut packed, mut loose) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for &i in &marks.grid[(cy.max(0) * MARK_CELLS + cx.max(0)) as usize] {
            let mk = &marks.marks[i as usize];
            let d = wrap_delta(m - mk.at, size);
            let (h, inside, rim) = mark_effect(mk, d);
            dig = dig.min(h);
            raise = raise.max(h);
            packed = packed.max(inside);
            loose = loose.max(rim);
        }
        let disturbed = packed.max(loose);
        let ripple = 0.0008 * ripple_amt * ripple_shape * (1.0 - disturbed);
        // Lumps of loose sand kicked up and left: clods a few centimeters across.
        let lumps = fb(uv * 14.0, (14, 14), 3, 9);
        let cc = cell(uv * 70.0, (70, 70), 10);
        let clod = if cc.id < 0.3 {
            let r = 0.2 + 0.25 * cc.id / 0.3;
            (1.0 - smooth(r * 0.3, r, cc.d1)) * (0.5 + 0.5 * cc.id / 0.3)
        } else {
            0.0
        };
        // Pale where the sand lies loose and dry, darker where feet packed it.
        let mottle = fb(uv * 5.0, (5, 5), 3, 11) * 0.6 + fb(uv * 17.0, (17, 17), 2, 12) * 0.4;
        // Pebbles.
        let pc = cell(uv * 140.0, (140, 140), 8);
        let peb_r = 0.22 + 0.25 * pc.id;
        let pebble = if pc.id < 0.07 {
            1.0 - smooth(peb_r * 0.6, peb_r, pc.d1)
        } else {
            0.0
        };
        let height = 0.004 * low
            + 0.003 * lumps
            + 0.0012 * mid
            + 0.0004 * fine
            + 0.00025 * (speck - 0.5)
            + ripple
            + dig
            + raise
            + 0.002 * clod
            + 0.0025 * pebble;
        let warm = hex(0xcdae80);
        let pale = hex(0xddc7a2);
        let dark = hex(0xb59468);
        let mut a = mix(warm, pale, smooth(-0.2, 0.7, low * 0.8 + mid * 0.4));
        a = mix(a, dark, smooth(0.1, 0.8, -low * 0.7 + fine * 0.4) * 0.45);
        a *= 0.94 + 0.12 * speck2 + 0.02 * ripple_shape * ripple_amt;
        if speck > 0.993 {
            a = mix(a, hex(0x7b6650), 0.45);
        } else if speck < 0.006 {
            a = mix(a, hex(0xf2ead8), 0.45);
        }
        a *= 1.0 - 0.08 * packed + 0.07 * loose + 0.06 * clod;
        a *= 1.0 + 0.09 * mottle + 0.03 * lumps;
        a = mix(a, hex(0x7a5d48) * (0.7 + speck * 0.6), pebble * 0.85);
        let rough =
            (0.95 - 0.1 * packed - 0.25 * pebble + 0.03 * fine + 0.03 * clod).clamp(0.5, 1.0);
        let occ = (1.0 - 0.18 * packed + 0.05 * raise.min(0.0)).clamp(0.6, 1.0);
        Texel {
            albedo: a,
            height,
            roughness: rough,
            metallic: 0.0,
            occlusion: occ,
        }
    })
}

/// Dressed sandstone: warm bedding layers, claw-chisel marks, pores, hairline cracks and
/// weathered patches (2 m repeat). Tint it with vertex colors per block.
pub fn stone_set() -> TextureSet {
    bake_par(1024, span::STONE, stone_texel)
}

fn stone_texel(uv: Vec2) -> Texel {
    let big = fb(uv * 3.0, (3, 3), 4, 11);
    let mid = fb(uv * 12.0, (12, 12), 3, 12);
    let fine = fb(uv * 48.0, (48, 48), 2, 13);
    let grain = pn(uv * 320.0, (320, 320), 14);
    let warp = fb(uv * 2.0, (2, 2), 3, 16) * 0.05;
    let bed = ((uv.y + warp) * 23.0 * TAU).sin() * 0.5 + 0.5;
    let bed2 = ((uv.y + warp * 1.3 + fine * 0.004) * 71.0 * TAU).sin();
    // Claw-chisel marks: patches of parallel grooves in different directions.
    let c = cell(uv * 7.0, (7, 7), 17);
    let ang = c.id * PI;
    let dir = Vec2::new(ang.cos(), ang.sin());
    let rel_m = c.rel * (span::STONE / 7.0);
    let tool_wave = (rel_m.dot(dir) / 0.013 * TAU + fine * 2.0).sin();
    let tool_mask = smooth(0.0, 0.2, c.d2 - c.d1) * smooth(-0.3, 0.4, mid) * 0.9;
    // Pores and pits.
    let pc = cell(uv * 110.0, (110, 110), 18);
    let pit = if pc.id < 0.3 {
        1.0 - smooth(0.1, 0.3 * (0.5 + pc.id), pc.d1)
    } else {
        0.0
    };
    // Hairline cracks in some areas.
    let cw = fb(uv * 6.0, (6, 6), 2, 19) * 0.15;
    let ridge = 1.0 - (pn(uv * 5.0 + Vec2::splat(cw), (5, 5), 20) * 2.2).abs();
    let crack = smooth(0.955, 0.995, ridge) * smooth(0.1, 0.6, big + 0.2 * mid);
    let halo = smooth(0.85, 0.99, ridge) * smooth(0.1, 0.6, big + 0.2 * mid);
    let cream = hex(0xd6c3a0);
    let ochre = hex(0xc49d6a);
    let grey = hex(0xaba497);
    let mut a = mix(cream, ochre, smooth(-0.4, 0.7, big) * 0.65 + bed * 0.18);
    a = mix(a, grey, smooth(0.15, 0.75, mid * 0.7 + fine * 0.3) * 0.35);
    a *= 1.0 + 0.025 * bed2 + 0.05 * fine + 0.035 * grain;
    // Darker weathered blotches.
    let blotch = smooth(0.35, 0.8, fb(uv * 5.0, (5, 5), 3, 21));
    a = mix(a, a * Vec3::new(0.72, 0.7, 0.66), blotch * 0.5);
    a *= 1.0 - 0.45 * pit - 0.2 * crack - 0.05 * halo;
    let height = 0.0022 * big
        + 0.0008 * mid
        + 0.0003 * fine
        + 0.0001 * grain
        + 0.0003 * tool_wave * tool_mask
        + 0.0003 * bed
        - 0.0012 * pit
        - 0.0004 * crack;
    Texel {
        albedo: a,
        height,
        roughness: (0.83 + 0.07 * fine + 0.1 * pit - 0.04 * tool_mask + 0.05 * blotch)
            .clamp(0.5, 1.0),
        metallic: 0.0,
        occlusion: (1.0 - 0.45 * pit - 0.25 * crack - 0.08 * (1.0 - bed)).clamp(0.3, 1.0),
    }
}

/// Weathered timber, grain along u: rings, raised late wood, knots, checks, gray weathering
/// (2 m repeat).
pub fn wood_set() -> TextureSet {
    bake_par(1024, span::WOOD, |uv| {
        let warp = fb(uv * Vec2::new(2.0, 10.0), (2, 10), 3, 31) * 0.5
            + fb(uv * Vec2::new(6.0, 40.0), (6, 40), 2, 32) * 0.12;
        let cathedral = (uv.x * TAU * 2.0).sin() * 1.6 + (uv.x * TAU * 3.0 + 1.0).sin() * 0.7;
        // Knots shift the rings around them.
        let kc = cell(uv * Vec2::new(4.0, 12.0), (4, 12), 33);
        let knot_r = 0.33 + 0.2 * kc.id;
        let is_knot = kc.id < 0.12;
        let kd = kc.rel * Vec2::new(1.0, 0.33);
        let knot = if is_knot {
            1.0 - smooth(0.0, knot_r, kd.length())
        } else {
            0.0
        };
        let rings_phase = uv.y * 110.0 + warp * 9.0 + cathedral + knot * 5.0;
        let t = rings_phase.rem_euclid(1.0);
        let late = smooth(0.62, 0.8, t) * (1.0 - smooth(0.9, 1.0, t));
        let streak = pn(uv * Vec2::new(12.0, 600.0), (12, 600), 34);
        let pores = pn(uv * Vec2::new(40.0, 900.0), (40, 900), 35);
        // Checks: long thin splits along the grain.
        let ck = 1.0 - (pn(uv * Vec2::new(3.0, 160.0), (3, 160), 36) * 2.5).abs();
        let check = smooth(0.95, 0.995, ck) * smooth(-0.1, 0.4, fb(uv * 4.0, (4, 4), 2, 37));
        let weather = smooth(-0.5, 0.6, fb(uv * Vec2::new(3.0, 6.0), (3, 6), 3, 38));
        let brown = hex(0x8a6444);
        let gray = hex(0x8d8378);
        let mut a = mix(brown, gray, 0.25 + 0.55 * weather);
        a *= 1.0 - 0.28 * late + 0.06 * streak + 0.04 * pores;
        a = mix(a, hex(0x3b2a1e), knot * 0.85);
        a *= 1.0 - 0.6 * check;
        Texel {
            albedo: a,
            height: 0.0005 * late + 0.00015 * streak - 0.0012 * check + 0.0004 * knot
                - 0.0001 * pores.abs(),
            roughness: (0.78 + 0.1 * weather + 0.08 * check - 0.05 * late).clamp(0.4, 1.0),
            metallic: 0.0,
            occlusion: (1.0 - 0.5 * check - 0.1 * late).clamp(0.3, 1.0),
        }
    })
}

/// Forged iron: hammer dimples, dark scale, rust blooms and pitting (1 m repeat).
pub fn iron_set() -> TextureSet {
    bake_par(512, span::IRON, |uv| {
        let hc = cell(uv * 36.0, (36, 36), 41);
        let dimple = (1.0 - (hc.d1 * 1.4).min(1.0).powi(2)) * (0.5 + hc.id);
        let rust_n = fb(uv * 4.0, (4, 4), 4, 42) + 0.45 * fb(uv * 18.0, (18, 18), 3, 43);
        let rust = smooth(0.2, 0.55, rust_n);
        let pc = cell(uv * 90.0, (90, 90), 44);
        let pit = if pc.id < 0.4 {
            1.0 - smooth(0.05, 0.25, pc.d1)
        } else {
            0.0
        } * rust;
        let fine = fb(uv * 60.0, (60, 60), 2, 45);
        let metal = hex(0x55534f) * (0.85 + 0.25 * fine);
        let rust_c = mix(hex(0x6e3416), hex(0x9a5a2a), smooth(-0.3, 0.6, fine));
        let a = mix(metal, rust_c, rust) * (1.0 - 0.4 * pit);
        Texel {
            albedo: a,
            height: -0.0004 * dimple + 0.0003 * rust_n.max(0.0) - 0.0006 * pit + 0.00005 * fine,
            roughness: mix(Vec3::splat(0.5 + 0.12 * fine), Vec3::splat(0.92), rust).x,
            metallic: 0.9 * (1.0 - rust),
            occlusion: (1.0 - 0.5 * pit).clamp(0.3, 1.0),
        }
    })
}

/// Worn leather: pebbled grain, creases, rubbed lighter spots (0.5 m repeat).
pub fn leather_set() -> TextureSet {
    bake_par(256, span::LEATHER, |uv| {
        let gc = cell(uv * 48.0, (48, 48), 51);
        let grain = smooth(0.0, 0.5, gc.d2 - gc.d1);
        let crease = smooth(0.92, 0.99, 1.0 - (pn(uv * 6.0, (6, 6), 52) * 2.4).abs());
        let rub = smooth(0.1, 0.7, fb(uv * 3.0, (3, 3), 3, 53));
        let a = mix(hex(0x5a3a22), hex(0x8a6a4a), rub * 0.6)
            * (0.92 + 0.08 * grain)
            * (1.0 - 0.35 * crease);
        Texel {
            albedo: a,
            height: 0.0002 * grain - 0.0005 * crease,
            roughness: 0.62 - 0.12 * rub + 0.1 * crease,
            metallic: 0.0,
            occlusion: 1.0 - 0.3 * crease,
        }
    })
}

/// Woven rush mat: a two-over-two basket weave of straw strands, darker where trodden
/// (1 m repeat).
pub fn mat_set() -> TextureSet {
    bake_par(512, span::MAT, |uv| {
        let n = 40.0;
        let p = uv * n;
        let (ix, iy) = (p.x.floor() as i32, p.y.floor() as i32);
        let f = p - p.floor();
        // Which strand is on top here: blocks of 2×2 alternate.
        let horizontal_top = ((ix.div_euclid(2) + iy.div_euclid(2)) & 1) == 0;
        let (across, along, id) = if horizontal_top {
            (f.y, f.x, iy)
        } else {
            (f.x, f.y, 1000 + ix)
        };
        let profile = (1.0 - (across * 2.0 - 1.0).powi(2)).max(0.0).sqrt();
        let fiber = pn(Vec2::new(along * 2.0, id as f32 * 3.1), (0, 0), 61) * 0.5
            + pn(
                uv * Vec2::new(
                    if horizontal_top { 20.0 } else { 300.0 },
                    if horizontal_top { 300.0 } else { 20.0 },
                ),
                (
                    if horizontal_top { 20 } else { 300 },
                    if horizontal_top { 300 } else { 20 },
                ),
                62,
            ) * 0.5;
        let strand_tone = r2(id, 0, 63);
        let worn = smooth(-0.2, 0.6, fb(uv * 2.0, (2, 2), 3, 64));
        let mut a = mix(hex(0xb59a5e), hex(0x8a7444), strand_tone * 0.7);
        a *= 0.8 + 0.2 * profile + 0.08 * fiber;
        a = mix(a, a * Vec3::new(0.62, 0.58, 0.52), worn * 0.55);
        Texel {
            albedo: a,
            height: 0.0015 * profile + 0.0002 * fiber,
            roughness: 0.82 + 0.08 * worn,
            metallic: 0.0,
            occlusion: 0.65 + 0.35 * profile,
        }
    })
}

/// Coal bed: lumps with pale ash on their tops (albedo), and the glow between and inside
/// them as an emissive image (sRGB).
pub fn coal_set() -> (TextureSet, Image) {
    let glow = |uv: Vec2| -> f32 {
        let c = cell(uv * 14.0, (14, 14), 71);
        let seam = 1.0 - smooth(0.0, 0.16, c.d2 - c.d1);
        let flicker = smooth(-0.3, 0.6, fb(uv * 6.0, (6, 6), 3, 72));
        let hot_lump = if c.id < 0.3 {
            (1.0 - smooth(0.1, 0.6, c.d1)) * 0.6
        } else {
            0.0
        };
        (seam * (0.35 + 0.65 * flicker) + hot_lump * flicker).clamp(0.0, 1.0)
    };
    let set = bake_par(512, span::COAL, |uv| {
        let c = cell(uv * 14.0, (14, 14), 71);
        let dome = (1.0 - (c.d1 * 1.3).min(1.0).powi(2)).max(0.0);
        let ash = smooth(0.5, 0.9, dome) * smooth(-0.2, 0.5, fb(uv * 9.0, (9, 9), 2, 73));
        let g = glow(uv);
        let a = mix(hex(0x1c1a19), hex(0x9a948c), ash * 0.8) * (1.0 - 0.6 * g);
        Texel {
            albedo: a,
            height: 0.012 * dome + 0.002 * fb(uv * 40.0, (40, 40), 2, 74),
            roughness: 0.9,
            metallic: 0.0,
            occlusion: 0.5 + 0.5 * dome,
        }
    });
    let emissive = image_par(512, 512, |uv| {
        let g = glow(uv);
        (fire_color(0.25 + 0.6 * g) * g * g).extend(1.0)
    });
    (set, emissive)
}

// ============================================================================== fire

/// Peak emission of the flames, cd/m² (the flame image scales it down toward the tips).
const FLAME_GLOW: f32 = 100_000.0;
const FLAME_KEY: &str = "npc/arena/flame";

/// Fire colors by temperature 0..1: dull red, orange, yellow, a pale yellow core (linear).
fn fire_color(t: f32) -> Vec3 {
    const STOPS: [(f32, [f32; 3]); 5] = [
        (0.0, [0.3, 0.025, 0.0]),
        (0.35, [0.95, 0.2, 0.02]),
        (0.6, [1.0, 0.45, 0.07]),
        (0.82, [1.0, 0.7, 0.26]),
        (1.0, [1.0, 0.9, 0.62]),
    ];
    let t = t.clamp(0.0, 1.0);
    for w in STOPS.windows(2) {
        let ((t0, a), (t1, b)) = (w[0], w[1]);
        if t <= t1 {
            return mix(Vec3::from(a), Vec3::from(b), (t - t0) / (t1 - t0));
        }
    }
    Vec3::from(STOPS[4].1)
}

/// A tongue of flame on a sheet (x −1..1 across, y 0..1 up): root position, sideways drift
/// at the tip, height and widest half-width.
#[derive(Debug, Clone, Copy)]
struct Tongue {
    x: f32,
    drift: f32,
    height: f32,
    width: f32,
}

/// A tall tongue in the middle and smaller ones licking up beside it.
fn tongues(seed: u64) -> Vec<Tongue> {
    let mut rng = Rng::new(seed);
    let mut v = vec![Tongue {
        x: rng.range(-0.08, 0.08),
        drift: rng.range(-0.15, 0.15),
        height: rng.range(0.9, 0.97),
        width: 0.62,
    }];
    for k in 0..5 {
        let side = if k % 2 == 0 { -1.0 } else { 1.0 };
        let off = rng.range(0.2, 0.68);
        v.push(Tongue {
            x: side * off,
            drift: side * rng.range(-0.1, 0.2),
            height: rng.range(0.35, 0.8) * (1.0 - 0.4 * off),
            width: rng.range(0.22, 0.36),
        });
    }
    v
}

/// One texel of a flame sheet: linear emission (1 = the hottest) and coverage.
fn flame_texel(x: f32, y: f32, tongues: &[Tongue], seed: u32) -> Vec4 {
    // Turbulence grows with height: the tongues sway and tear apart toward their tips.
    let sway = 0.26 * y.powf(1.2) * fb(Vec2::new(x * 1.1 + 3.0, y * 2.4), (0, 0), 3, seed)
        + 0.07 * y.sqrt() * pn(Vec2::new(x * 5.0, y * 8.0), (0, 0), seed ^ 3);
    let xd = x + sway;
    // A rolling body over the whole bed that the tongues rise from.
    let body_h = 0.3 + 0.08 * pn(Vec2::new(x * 2.5, 0.5), (0, 0), seed ^ 5);
    let b = (y / body_h).min(1.0);
    let qb = 1.0 - xd.abs() / (0.92 * (1.0 - b * b).sqrt()).max(1e-3);
    let mut cover = smooth(0.0, 0.5, qb) * (1.0 - smooth(0.6, 1.0, b));
    let mut heat = smooth(0.1, 0.8, qb) * (1.0 - b) * 0.9;
    for t in tongues {
        let s = y / t.height;
        if s >= 1.0 {
            continue;
        }
        let cx = t.x + t.drift * s * s;
        let hw = t.width * (1.0 - s).powf(0.65) * (0.75 + 0.25 * smooth(0.0, 0.3, s));
        let q = 1.0 - (xd - cx).abs() / hw.max(1e-3);
        if q <= 0.0 {
            continue;
        }
        let d = smooth(0.0, 0.8, q) * (1.0 - smooth(0.75, 1.0, s));
        cover = 1.0 - (1.0 - cover) * (1.0 - d);
        heat = heat.max(smooth(0.2, 1.0, q) * (1.0 - s).powf(0.6));
    }
    // Torn wisps near the tips, streaks inside, nothing at the sheet's edges.
    let tear = fb(Vec2::new(xd * 3.0, y * 1.6 - 0.3), (0, 0), 3, seed ^ 7);
    cover *= smooth(-0.3, 0.2, tear + 0.75 - 1.1 * y);
    cover *= 0.88 + 0.17 * pn(Vec2::new(xd * 9.0, y * 2.5), (0, 0), seed ^ 11);
    cover *= 1.0 - smooth(0.8, 0.97, x.abs());
    let t = (cover * (0.45 + 0.55 * heat) * (1.08 - 0.55 * y)).clamp(0.0, 1.0);
    // Dim, cool edges are thin, so they glow without darkening what is behind them, and the
    // root fades out where the sheets dip into the coals.
    let alpha =
        smooth(0.02, 0.4, cover) * smooth(0.03, 0.16, y) * (0.3 + 0.7 * smooth(0.05, 0.45, t));
    (fire_color(t) * (0.04 + 0.96 * t * t)).extend(alpha.clamp(0.0, 1.0))
}

/// Two flame sheets side by side (u 0..½ and ½..1, v 0 at the root), each a few licking
/// tongues: a pale yellow core through orange to dull red, torn tips. RGB is the emission
/// (sRGB) and alpha the shape, so one image serves as both.
pub fn flame_image() -> Image {
    let sheets = [tongues(31), tongues(47)];
    image_par(512, 512, |uv| {
        let k = (uv.x >= 0.5) as usize;
        let x = (uv.x * 2.0 - k as f32) * 2.0 - 1.0;
        flame_texel(x, uv.y, &sheets[k], 90 + k as u32 * 13)
    })
}

/// A brazier's fire (origin at the root, on the coals): crossed sheets of [`flame_image`],
/// curved, leaning and twisting as they rise, three tall ones through the middle and smaller
/// licks around them.
pub fn flame_mesh(seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut m = Mesh::default();
    let mut card = |rng: &mut Rng, at: Vec3, yaw: f32, size: Vec2| {
        let twist = rng.range(-0.5, 0.5);
        let lean = Vec3::new(rng.range(-0.06, 0.06), 0.0, rng.range(-0.06, 0.06));
        let bend = rng.range(-0.12, 0.12);
        let sheet = (rng.f() < 0.5) as u32 as f32;
        let flip = rng.f() < 0.5;
        let (nx, ny) = (6u32, 12u32);
        let base = m.vertices.len() as u32;
        for j in 0..=ny {
            let t = j as f32 / ny as f32;
            let (sa, ca) = (yaw + twist * t).sin_cos();
            for i in 0..=nx {
                let s = i as f32 / nx as f32 * 2.0 - 1.0;
                let (lx, lz) = (s * size.x * 0.5, bend * (s * s - 0.33) * size.x);
                let p = at
                    + Vec3::new(lx * ca + lz * sa, t * size.y - 0.06, lz * ca - lx * sa)
                    + lean * t * t;
                let u = if flip { 1.0 - s } else { 1.0 + s } * 0.5;
                let alpha = 1.0 - smooth(0.8, 1.0, s.abs());
                m.vertices.push(
                    Vertex::new(p, Vec3::new(sa, 0.0, ca), Vec2::new((sheet + u) * 0.5, t))
                        .with_color(Vec4::new(1.0, 1.0, 1.0, alpha)),
                );
            }
        }
        let w = nx + 1;
        for j in 0..ny {
            for i in 0..nx {
                let a = base + j * w + i;
                m.indices
                    .extend_from_slice(&[a, a + 1, a + w + 1, a, a + w + 1, a + w]);
            }
        }
    };
    let turn = rng.range(0.0, PI);
    for k in 0..3 {
        let at = Vec3::new(rng.range(-0.03, 0.03), 0.0, rng.range(-0.03, 0.03));
        let size = Vec2::new(0.85, 1.2 * rng.range(0.9, 1.05));
        card(&mut rng, at, turn + k as f32 * PI / 3.0, size);
    }
    for k in 0..3 {
        let a = turn + k as f32 * TAU / 3.0 + rng.range(-0.4, 0.4);
        let r = rng.range(0.14, 0.2);
        let at = Vec3::new(a.cos() * r, 0.0, -a.sin() * r);
        let size = Vec2::new(0.6, rng.range(0.55, 0.7));
        let yaw = a + FRAC_PI_2 + rng.range(-0.4, 0.4);
        card(&mut rng, at, yaw, size);
    }
    m
}

/// Makes a fire flicker ([`flicker_fires`]): a flame mesh stretches, sways and turns; a
/// light wavers around `lumens`. A brazier's flame and light share a seed, so the light
/// swells as the flame leaps.
#[derive(Component, Debug, Clone, Copy)]
pub struct Flicker {
    /// Phase offset, so neighboring fires don't pulse together.
    pub seed: f32,
    /// The light's mean output, lumens (unused on a flame).
    pub lumens: f32,
}

/// Animate every [`Flicker`].
pub fn flicker_fires(
    time: Res<Time>,
    mut q: Query<(&Flicker, &mut Transform, Option<&mut PointLight>)>,
) {
    let t = time.elapsed_secs();
    for (f, mut tf, light) in &mut q {
        let s = f.seed;
        let wave = |freq: f32, phase: f32| (t * freq + s * phase).sin();
        let leap = 0.5 * wave(7.3, 1.7) + 0.3 * wave(13.1, 2.9) + 0.2 * wave(21.7, 4.3);
        match light {
            Some(mut l) => l.intensity_lm = f.lumens * (0.85 + 0.15 * leap),
            None => {
                let squash = 1.0 - 0.05 * leap;
                tf.scale = Vec3::new(squash, 1.0 + 0.12 * leap, squash);
                tf.rotation = Quat::from_rotation_y(s * 2.4 + 0.35 * (t * 0.9 + s).sin())
                    * Quat::from_rotation_x(0.05 * wave(3.7, 1.1))
                    * Quat::from_rotation_z(0.05 * wave(4.3, 2.2));
            }
        }
    }
}

/// Which team a banner is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Team {
    /// Crimson with a golden sun in a laurel wreath (the north squad).
    Red,
    /// Deep blue with a silver crescent and stars in a laurel wreath (the south squad).
    Blue,
}

/// Banner cloth for `team`: heavy wool with an embroidered border, chevron hem and emblem.
/// u spans the 1.5 m width, v the 3.4 m drop (top at v = 0).
pub fn banner_set(team: Team) -> TextureSet {
    let (cloth, thread, thread_dark) = match team {
        Team::Red => (hex(0x7a1012), hex(0xd8a640), hex(0x8a5a18)),
        Team::Blue => (hex(0x13245e), hex(0xd9dde2), hex(0x7f8794)),
    };
    let (w, h) = (1.5f32, 3.4f32);
    bake_par(512, 2.0, |uv| {
        let p = Vec2::new(uv.x * w, uv.y * h);
        let weave = ((uv.x * 512.0 * PI).sin() * (uv.y * 512.0 * PI).sin()) * 0.5;
        let nap = fb(uv * Vec2::new(8.0, 16.0), (8, 16), 3, 81);
        // Border: an outer gold band with a running chevron, an inner thin line.
        let edge = p.x.min(w - p.x).min(p.y);
        let band = smooth(0.035, 0.04, edge) * (1.0 - smooth(0.11, 0.115, edge));
        let along = if p.x.min(w - p.x) < p.y { p.y } else { p.x };
        let chevron = ((along * 18.0 + (edge - 0.075).abs() * 18.0).rem_euclid(1.0) - 0.5).abs();
        let band_pattern = smooth(0.18, 0.22, chevron);
        let line = smooth(0.14, 0.145, edge) * (1.0 - smooth(0.155, 0.16, edge));
        // Chevron hem band near the bottom.
        let hem_y = h - 0.55;
        let hem = smooth(hem_y - 0.2, hem_y - 0.19, p.y)
            * (1.0 - smooth(hem_y + 0.01, hem_y + 0.02, p.y));
        let hem_pat = smooth(
            0.4,
            0.45,
            ((p.x * 4.0 + (p.y - hem_y) * 4.0).abs()).rem_euclid(1.0),
        ) * (1.0
            - smooth(
                0.85,
                0.9,
                ((p.x * 4.0 + (p.y - hem_y) * 4.0).abs()).rem_euclid(1.0),
            ));
        // Emblem centered 1.25 m below the top.
        let c = Vec2::new(w * 0.5, 1.3);
        let q = p - c;
        let r = q.length();
        let a = q.y.atan2(q.x);
        let emblem = match team {
            Team::Red => {
                // A sun: disc, ring and sixteen rays of alternating length.
                let disc = 1.0 - smooth(0.17, 0.175, r);
                let ring = smooth(0.205, 0.21, r) * (1.0 - smooth(0.225, 0.23, r));
                let k = (a / TAU * 16.0).rem_euclid(1.0);
                let long = ((a / TAU * 16.0).floor() as i32 & 1) == 0;
                let ray_len = if long { 0.43 } else { 0.35 };
                let tri = (k - 0.5).abs() * 2.0;
                let along = ((r - 0.245) / (ray_len - 0.245)).clamp(0.0, 1.0);
                let width = 0.75 * (1.0 - along);
                let ray = if r > 0.245 && r < ray_len {
                    1.0 - smooth(width - 0.06, width, tri)
                } else {
                    0.0
                };
                // A face: two eyes and a smile cut from the disc.
                let eyes = (1.0 - smooth(0.02, 0.025, (q - Vec2::new(-0.06, -0.03)).length()))
                    .max(1.0 - smooth(0.02, 0.025, (q - Vec2::new(0.06, -0.03)).length()));
                let mouth = (1.0 - smooth(0.012, 0.017, (q.length() - 0.09).abs()))
                    * smooth(0.03, 0.05, q.y);
                (disc * (1.0 - eyes.max(mouth) * 0.85)).max(ring).max(ray)
            }
            Team::Blue => {
                // A crescent and three stars.
                let moon = (1.0 - smooth(0.25, 0.255, r))
                    * smooth(0.21, 0.215, (q - Vec2::new(0.09, -0.05)).length());
                let star = |c: Vec2, s: f32| {
                    let d = p - c;
                    let ang = d.y.atan2(d.x) + FRAC_PI_2;
                    let k = ((ang / TAU * 5.0).rem_euclid(1.0) - 0.5).abs() * 2.0;
                    let rad = s * (0.45 + 0.55 * k);
                    1.0 - smooth(rad - 0.004, rad, d.length())
                };
                moon.max(star(c + Vec2::new(0.13, -0.08), 0.07))
                    .max(star(c + Vec2::new(0.2, 0.1), 0.05))
                    .max(star(c + Vec2::new(0.02, 0.2), 0.045))
            }
        };
        // Laurel wreath: leaves along an arc below the emblem on both sides.
        let mut wreath: f32 = 0.0;
        for side in [-1.0f32, 1.0] {
            for k in 0..9 {
                let t = k as f32 / 8.0;
                let ang = -FRAC_PI_2 - side * (0.35 + t * 2.0);
                let base = c + Vec2::new(ang.cos(), -ang.sin()) * 0.5;
                let tangent = Vec2::new(-ang.sin(), -ang.cos()) * -side;
                let normal = Vec2::new(tangent.y, -tangent.x);
                for (off, tilt) in [(0.04f32, 0.6f32), (-0.04, -0.6)] {
                    let leaf_c = base + normal * off;
                    let dir = (tangent + normal * tilt).normalize();
                    let d = p - leaf_c;
                    let l = Vec2::new(d.dot(dir), d.dot(Vec2::new(-dir.y, dir.x)));
                    let e = ellipse(l, Vec2::ZERO, Vec2::new(0.055, 0.02) * (1.0 - 0.3 * t));
                    wreath = wreath.max(1.0 - smooth(-0.002, 0.002, e));
                }
            }
        }
        let stem = {
            let d = r_arc(p - c, 0.5);
            (1.0 - smooth(0.006, 0.01, d)) * smooth(0.05, 0.1, (p - c).y)
        };
        let gold = band
            .max(line)
            .max(emblem)
            .max(wreath)
            .max(stem)
            .max(hem * hem_pat);
        let stitch = ((p.x + p.y) * 300.0).sin() * 0.5 + 0.5;
        let thread_c = mix(thread_dark, thread, 0.55 + 0.45 * stitch) * (0.9 + 0.1 * nap);
        let base = cloth * (0.9 + 0.08 * nap + 0.04 * weave) * (1.0 - 0.2 * smooth(2.0, h, p.y));
        let band_c = mix(thread_c, thread_c * 0.6, band * (1.0 - band_pattern));
        let a = mix(base, band_c, gold);
        Texel {
            albedo: a,
            height: 0.0004 * weave + 0.0003 * nap + 0.0009 * gold + 0.0002 * stitch * gold,
            roughness: mix(Vec3::splat(0.9), Vec3::splat(0.45), gold).x,
            metallic: 0.55 * gold * if team == Team::Red { 1.0 } else { 0.8 },
            occlusion: 1.0 - 0.1 * (1.0 - gold) * (1.0 - weave),
        }
    })
}

/// Distance from `q` to the lower half of a circle of radius `r` (the wreath stem).
fn r_arc(q: Vec2, r: f32) -> f32 {
    (q.length() - r).abs()
}

/// Worn red steel drum paint with yellow-black hazard stripes, flame placards, scratches to
/// bare steel, rust streaks and dust. Layout: v 0..0.68 wraps around the drum (u = angle,
/// v = 0 at the top rim), the square u 0..0.32, v 0.68..1 is the lid.
pub fn barrel_set() -> TextureSet {
    const SIDE_V: f32 = 0.68;
    bake_par(768, 2.0, |uv| {
        if uv.y < SIDE_V {
            let y = 1.0 - uv.y / SIDE_V; // 0 bottom .. 1 top (meters)
            let around = uv.x * 2.01; // meters around
            barrel_side(uv, around, y)
        } else if uv.x < 0.32 {
            let c = Vec2::new(0.16, 0.84);
            let q = (uv - c) * 2.0; // meters on the lid, radius 0.32
            barrel_lid(uv, q)
        } else {
            Texel {
                albedo: hex(0x8a1a12),
                ..Default::default()
            }
        }
    })
}

fn paint_wear(uv: Vec2, edge: f32, seed: u32) -> (f32, f32) {
    // (bare: 0..1 paint gone, rust: 0..1)
    let chips = fb(uv * 30.0, (30, 30), 3, seed) + 0.6 * fb(uv * 120.0, (120, 120), 2, seed + 1);
    let scratch_a = 1.0 - (pn(uv * Vec2::new(60.0, 6.0), (60, 6), seed + 2) * 3.0).abs();
    let scratch_b = 1.0 - (pn(uv * Vec2::new(8.0, 90.0), (8, 90), seed + 3) * 3.0).abs();
    let scratch = smooth(0.97, 0.995, scratch_a.max(scratch_b))
        * smooth(-0.2, 0.3, fb(uv * 5.0, (5, 5), 2, seed + 4));
    let bare = smooth(0.62 - edge * 0.9, 0.7 - edge * 0.9, chips).max(scratch);
    let rust = bare * smooth(-0.1, 0.5, fb(uv * 12.0, (12, 12), 3, seed + 5));
    (bare, rust)
}

fn barrel_side(uv: Vec2, around: f32, y: f32) -> Texel {
    let red = hex(0xa3201a);
    // Wear near the rolled rims and the two hoops.
    let rims = [0.0f32, 1.0, 0.33, 0.67];
    let near_rim = rims
        .iter()
        .map(|r| 1.0 - smooth(0.0, 0.05, (y - r).abs()))
        .fold(0.0, f32::max);
    let (bare, rust) = paint_wear(uv, near_rim * 0.35, 91);
    // Hazard band between the hoops.
    let in_band = smooth(0.415, 0.42, y) * (1.0 - smooth(0.58, 0.585, y));
    let stripe = ((around + y) / 0.14).rem_euclid(1.0) < 0.5;
    let band_c = if stripe { hex(0xd9a50e) } else { hex(0x161616) };
    // Two flame placards in the top section, front and back.
    let mut placard = 0.0f32;
    let mut symbol = 0.0f32;
    for cx in [0.25f32, 0.75] {
        let d = Vec2::new((uv.x - cx) * 2.01, y - 0.8);
        let dia = d.x.abs() + d.y.abs();
        placard = placard.max(1.0 - smooth(0.115, 0.12, dia));
        let border = smooth(0.1, 0.105, dia) * (1.0 - smooth(0.115, 0.12, dia));
        // Flame: a teardrop with a notch.
        let f = Vec2::new(d.x, d.y + 0.012);
        let body = ellipse(f, Vec2::new(0.0, -0.012), Vec2::new(0.035, 0.03));
        let tip = ellipse(f, Vec2::new(0.006, 0.02), Vec2::new(0.016, 0.045));
        let inner = ellipse(f, Vec2::new(0.006, -0.01), Vec2::new(0.012, 0.016));
        let flame = (1.0 - smooth(-0.002, 0.002, sdf::smin(body, tip, 0.02)))
            * smooth(-0.002, 0.002, inner);
        let base_line = (1.0 - smooth(0.004, 0.007, (d.y + 0.05).abs()))
            * (1.0 - smooth(0.04, 0.045, d.x.abs()));
        symbol = symbol.max(border).max(flame).max(base_line);
    }
    let mut paint_c = mix(red, band_c, in_band);
    paint_c = mix(paint_c, hex(0xe0820e), placard);
    paint_c = mix(paint_c, hex(0x111111), symbol * placard.max(symbol));
    let fade = fb(uv * 4.0, (4, 4), 3, 92);
    paint_c *= 0.9 + 0.12 * fade;
    // Rust streaks running down from the hoops and chips.
    let streak_n = pn(uv * Vec2::new(70.0, 3.0), (70, 3), 93) * 0.5 + 0.5;
    let streak = smooth(0.62, 0.85, streak_n) * smooth(0.1, 0.6, near_rim.max(rust) + 0.25) * 0.55;
    let steel = hex(0x8c8c8e);
    let rust_c = mix(hex(0x5c2a12), hex(0x8b4a22), fade * 0.5 + 0.5);
    let mut a = mix(paint_c, steel, bare);
    a = mix(a, rust_c, rust.max(streak * (1.0 - bare)));
    // Sand dust toward the bottom.
    let dust = (1.0 - smooth(0.0, 0.2, y)) * (0.6 + 0.4 * fade);
    a = mix(a, hex(0xb59a72), dust * 0.6);
    let metal = bare * (1.0 - rust);
    Texel {
        albedo: a,
        height: 0.00012 * (1.0 - bare) - 0.00025 * rust + 0.00004 * fade,
        roughness: (0.42 + 0.15 * fade + 0.45 * rust + 0.35 * dust - 0.1 * metal).clamp(0.2, 1.0),
        metallic: metal,
        occlusion: 1.0 - 0.15 * rust,
    }
}

fn barrel_lid(uv: Vec2, q: Vec2) -> Texel {
    let r = q.length();
    let (bare, rust) = paint_wear(uv * 3.0, smooth(0.26, 0.31, r) * 0.4, 95);
    let rings = [0.12f32, 0.22];
    let emboss = rings
        .iter()
        .map(|c| 1.0 - smooth(0.004, 0.009, (r - c).abs()))
        .fold(0.0, f32::max);
    let bung = |c: Vec2, rad: f32| 1.0 - smooth(rad - 0.004, rad, (q - c).length());
    let bungs = bung(Vec2::new(0.19, 0.0), 0.035).max(bung(Vec2::new(-0.2, 0.06), 0.022));
    let fade = fb(uv * 12.0, (12, 12), 3, 96);
    let mut a = hex(0xa3201a) * (0.9 + 0.1 * fade);
    a = mix(a, hex(0x8c8c8e), bare);
    a = mix(a, hex(0x6a3014), rust);
    // Puddle stains and grit on the lid.
    let grime = smooth(0.2, 0.7, fb(uv * 6.0, (6, 6), 3, 97));
    a = mix(a, hex(0x4a3a2a), grime * 0.35);
    Texel {
        albedo: a,
        height: 0.0012 * emboss + 0.003 * bungs - 0.0002 * rust,
        roughness: (0.45 + 0.4 * rust + 0.2 * grime).clamp(0.2, 1.0),
        metallic: bare * (1.0 - rust),
        occlusion: 1.0 - 0.2 * grime,
    }
}

// ============================================================================== stone kit

/// Value noise in 3D, about −1..1 (cheap: no trigonometry), for sculpting.
#[inline(always)]
fn vn3(p: Vec3, seed: u32) -> f32 {
    let i = p.floor();
    let f = p - i;
    let (x, y, z) = (i.x as i32, i.y as i32, i.z as i32);
    let h = |dx: i32, dy: i32, dz: i32| rand3(x + dx, y + dy, z + dz, seed) * 2.0 - 1.0;
    let (u, v, w) = (fade(f.x), fade(f.y), fade(f.z));
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    l(
        l(
            l(h(0, 0, 0), h(1, 0, 0), u),
            l(h(0, 1, 0), h(1, 1, 0), u),
            v,
        ),
        l(
            l(h(0, 0, 1), h(1, 0, 1), u),
            l(h(0, 1, 1), h(1, 1, 1), u),
            v,
        ),
        w,
    )
}

/// Options for [`chipped_block`].
#[derive(Debug, Clone, Copy)]
pub struct Chip {
    /// Edge rounding before chipping, meters.
    pub radius: f32,
    /// Meshing resolution, meters (triangles grow with area / voxel²).
    pub voxel: f32,
    /// Deepest edge chip, meters (most are shallower).
    pub depth: f32,
    /// Up to this many broken-off corners.
    pub breaks: u32,
    pub seed: u64,
    /// Leave the bottom face out (it sits on the ground or on another block).
    pub open_bottom: bool,
}

impl Chip {
    pub fn new(seed: u64) -> Self {
        Self {
            radius: 0.014,
            voxel: 0.03,
            depth: 0.04,
            breaks: 1,
            seed,
            open_bottom: true,
        }
    }
}

/// A cut stone block centered on the origin: crisp, slightly rounded arrises with shallow
/// conchoidal chips scooped out of them, sometimes a corner broken off along a rough fracture
/// plane, and faces that undulate a hair. Meshed from a distance field, so vertex colors carry
/// crevice occlusion; texture coordinates are box-projected meters.
pub fn chipped_block(half: Vec3, c: Chip) -> Mesh {
    let mut rng = Rng::new(c.seed);
    let seed = (c.seed as u32).wrapping_mul(2654435761);
    let r = c.radius.min(half.min_element() * 0.8);
    let mut cuts: Vec<(Vec3, f32)> = Vec::new();
    for _ in 0..c.breaks {
        if rng.f() < 0.35 {
            continue;
        }
        let s = Vec3::new(
            if rng.f() < 0.5 { -1.0 } else { 1.0 },
            if rng.f() < 0.75 { 1.0 } else { -1.0 },
            if rng.f() < 0.5 { -1.0 } else { 1.0 },
        );
        let corner = s * half;
        let n = (s * Vec3::new(
            rng.range(0.6, 1.4),
            rng.range(0.6, 1.4),
            rng.range(0.6, 1.4),
        ))
        .normalize();
        let depth = rng.range(0.035, 0.11).min(half.min_element() * 0.9);
        cuts.push((n, corner.dot(n) - depth));
    }
    // Conchoidal chips: shallow spherical scoops bitten out of the edges, more of them on the
    // top edges (knocked in handling) than on the bottom ones.
    let mut scoops: Vec<(Vec3, f32)> = Vec::new();
    let edge_len = 4.0 * (half.x + half.y + half.z) * 2.0;
    let n_chips = (edge_len * 2.2).round() as usize;
    for _ in 0..n_chips {
        let axis = (rng.f() * 3.0) as usize % 3;
        let (b, cc) = ((axis + 1) % 3, (axis + 2) % 3);
        let mut s = [0.0f32; 3];
        s[b] = if rng.f() < 0.5 { -1.0 } else { 1.0 };
        s[cc] = if rng.f() < 0.5 { -1.0 } else { 1.0 };
        let mut e = Vec3::ZERO;
        e[axis] = rng.signed() * (half[axis] - 0.01).max(0.0);
        e[b] = s[b] * half[b];
        e[cc] = s[cc] * half[cc];
        if c.open_bottom && e.y < 0.0 && axis != 1 && rng.f() < 0.7 {
            continue;
        }
        let mut dir = Vec3::ZERO;
        dir[b] = s[b] * rng.range(0.35, 1.0);
        dir[cc] = s[cc] * rng.range(0.35, 1.0);
        let dir = dir.normalize();
        let depth = rng.range(0.006, c.depth.max(0.008)) * if rng.f() < 0.2 { 1.6 } else { 1.0 };
        let radius = depth * rng.range(2.2, 4.5);
        scoops.push((e + dir * (radius - depth), radius));
    }
    let f = move |p: Vec3| {
        let q = p.abs() - (half - Vec3::splat(r));
        let mut d = q.max(Vec3::ZERO).length() + q.max_element().min(0.0) - r;
        // A slight undulation of the dressed faces, no lumps.
        d += 0.0016 * vn3(p * 5.0, seed ^ 0x99) + 0.0005 * vn3(p * 23.0, seed ^ 0x77);
        for &(center, radius) in &scoops {
            let rough = 0.0015 * vn3(p * 60.0, seed ^ 0x2d);
            d = d.max(radius - p.distance(center) + rough);
        }
        for &(n, off) in &cuts {
            let rough = 0.004 * vn3(p * 24.0, seed ^ 0x33) + 0.0015 * vn3(p * 70.0, seed ^ 0x34);
            d = d.max(p.dot(n) - off + rough);
        }
        d
    };
    let mut min = -half - Vec3::splat(0.02);
    let max = half + Vec3::splat(0.02);
    if c.open_bottom {
        min.y = -half.y + c.voxel * 1.2;
    }
    sdf_mesh(
        &f,
        min,
        max,
        SdfOpts {
            voxel: c.voxel,
            occlusion: 0.9,
        },
        &|_, _| (Vec4::ONE, Vec2::ZERO),
    )
}

/// Stone tones for per-block vertex tints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Warm sandstone.
    Sand,
    /// Paler, cooler limestone (columns, curbs, inlays).
    Lime,
    /// Grimy, shadowed stone (drains, foundations).
    Grime,
    /// Dark mortar.
    Mortar,
}

fn tone(rng: &mut Rng, t: Tone) -> Vec4 {
    let w = rng.signed();
    let (v, hue) = match t {
        Tone::Sand => (
            rng.range(0.8, 1.0),
            Vec3::new(1.0, 0.97 + 0.03 * w, 0.93 + 0.06 * w),
        ),
        Tone::Lime => (
            rng.range(0.94, 1.06),
            Vec3::new(0.97, 0.99, 1.04 + 0.03 * w),
        ),
        Tone::Grime => (rng.range(0.5, 0.62), Vec3::new(1.0, 0.95, 0.88)),
        Tone::Mortar => (rng.range(0.55, 0.62), Vec3::new(0.95, 0.95, 0.95)),
    };
    (hue * v).extend(1.0)
}

/// Collects cut-stone blocks (rounded boxes with a tint, a texture offset and a hair of
/// misalignment each) into one mesh.
pub struct Mason {
    pub mesh: Mesh,
    pub rng: Rng,
    pub tone: Tone,
    /// Largest random tilt of a block, radians.
    pub wobble: f32,
    /// Left-out faces that keep their flat middle ([`box_lid`]): set it while laying blocks
    /// that overhang what they sit on (copings, treads, cornices), so their undersides close.
    pub lid: u8,
}

impl Mason {
    pub fn new(seed: u64, tone: Tone) -> Self {
        Self {
            mesh: Mesh::default(),
            rng: Rng::new(seed),
            tone,
            wobble: 0.003,
            lid: 0,
        }
    }

    /// A block filling `min..max` (grown 2–4 mm so neighbors close up).
    pub fn block(&mut self, min: Vec3, max: Vec3, o: BoxOpts) {
        let grow = self.rng.range(0.002, 0.004);
        let half = (max - min) * 0.5 + Vec3::splat(grow);
        let mut b = rounded_box(half, o);
        if self.lid & o.skip != 0 {
            b.merge(&box_lid(half, o, self.lid & o.skip));
        }
        joint_shade(&mut b);
        let center = (min + max) * 0.5;
        let w = self.wobble;
        let rot = Quat::from_euler(
            EulerRot::YXZ,
            self.rng.signed() * w,
            self.rng.signed() * w * 0.5,
            self.rng.signed() * w * 0.5,
        );
        self.add(b, Mat4::from_rotation_translation(rot, center));
    }

    /// Add a prepared piece with this mason's tint and a random texture offset.
    pub fn add(&mut self, mut piece: Mesh, m: Mat4) {
        let t = tone(&mut self.rng, self.tone);
        tint(&mut piece, t);
        offset_uv(
            &mut piece,
            Vec2::new(self.rng.range(0.0, 8.0), self.rng.range(0.0, 8.0)),
        );
        append(&mut self.mesh, &piece, m);
    }

    /// A course of blocks from `x0` to `x1` between heights `y0..y1` and depths `z0..z1`.
    pub fn course(
        &mut self,
        x: (f32, f32),
        y: (f32, f32),
        z: (f32, f32),
        len: (f32, f32),
        o: BoxOpts,
    ) {
        for (a, b) in run(&mut self.rng, x.0, x.1, len.0, len.1) {
            self.block(Vec3::new(a, y.0, z.0), Vec3::new(b, y.1, z.1), o);
        }
    }
}

/// Darken the rounded edges of a fresh [`rounded_box`] so joints between blocks read as dark
/// lines of dirt and shadow, and turn the upper arrises' normals toward the side faces: a
/// chamfer facing the sun would otherwise catch it as a bright line along every course.
fn joint_shade(m: &mut Mesh) {
    for v in &mut m.vertices {
        let n = Vec3::from(v.normal);
        if n.abs().max_element() >= 0.98 {
            continue;
        }
        let k = 0.55;
        v.color = (Vec4::from(v.color) * Vec4::new(k, k * 0.97, k * 0.94, 1.0)).to_array();
        let side = Vec3::new(n.x, 0.0, n.z);
        if n.y > 0.05 && side.length_squared() > 1e-4 {
            v.normal = (side.normalize() * 0.9 + Vec3::Y * 0.2)
                .normalize()
                .to_array();
        }
    }
}

/// Darken vertex colors near the ground (dust and splash), `height` meters up from `ground`.
fn grime(m: &mut Mesh, ground: f32, height: f32, strength: f32) {
    paint(m, |p, _| {
        let k = 1.0 - strength * (1.0 - smooth(ground, ground + height, p.y));
        Vec4::new(k, k * 0.98, k * 0.95, 1.0)
    });
}

const T5: f32 = 0.087_488_66; // tan 5°

/// Shear blocks laid out in straight coordinates (reference distance `zr`) so x = ±zr·tan 5°
/// lands on the edges of a 10° sector at every distance: joints become radial and
/// neighboring sectors meet exactly. Chords at constant z stay straight.
pub fn shear(m: &mut Mesh, zr: f32) {
    deform(m, |p| Vec3::new(p.x * p.z / zr, p.y, p.z));
}

/// Block edges for the stands and gates (seen from afar): a soft two-facet chamfer.
fn sb(r: f32) -> BoxOpts {
    BoxOpts::round(r).bevel(1)
}

/// Block edges for pieces seen up close: rounded.
fn pb(r: f32) -> BoxOpts {
    BoxOpts::round(r).bevel(2)
}

fn curb(ms: &mut Mason, xw: f32) {
    let old = ms.tone;
    ms.tone = Tone::Lime;
    ms.course(
        (-xw, xw),
        (-0.15, 0.12),
        (40.25, 40.75),
        (0.9, 1.4),
        sb(0.035).skip(NEG_Y),
    );
    ms.tone = old;
}

/// An arch of `voussoirs` wedge stones (semicircle centered `xc, ys` from radius `r` to `ro`
/// in the XY plane, depth `z`) and the coursed wall around it between `x0..x1` up to the last
/// of `rows` (heights above the springing; choose them so each row clears the arch).
#[allow(clippy::too_many_arguments)]
fn arch(
    ms: &mut Mason,
    xc: f32,
    ys: f32,
    r: f32,
    ro: f32,
    voussoirs: u32,
    z: (f32, f32),
    x: (f32, f32),
    rows: &[f32],
) {
    let rm = (r + ro) * 0.5;
    let th = (ro - r) * 0.5;
    let zc = (z.0 + z.1) * 0.5;
    let zh = (z.1 - z.0) * 0.5;
    for k in 0..voussoirs {
        let a0 = PI * k as f32 / voussoirs as f32;
        let a1 = PI * (k + 1) as f32 / voussoirs as f32;
        let am = (a0 + a1) * 0.5;
        let key = k == voussoirs / 2;
        let extra = if key { 0.1 } else { 0.0 };
        let rc = rm + extra * 0.5;
        let half = Vec3::new(
            (a1 - a0) * 0.5 * rc + 0.003,
            th + extra * 0.5,
            zh + if key { 0.025 } else { 0.0 },
        );
        let mut b = rounded_box(half, BoxOpts::round(0.03).step(0.3));
        joint_shade(&mut b);
        deform(&mut b, |p| {
            let t = am - p.x / rc;
            let rho = rc + p.y;
            Vec3::new(xc + rho * t.cos(), ys + rho * t.sin(), zc + p.z)
        });
        ms.add(b, Mat4::IDENTITY);
    }
    let x_in = |a: f32| (r * r - a * a).max(0.0).sqrt();
    let x_out = |a: f32| (ro * ro - a * a).max(0.0).sqrt();
    for w in rows.windows(2) {
        let (a0, a1) = (w[0], w[1]);
        let y = (ys + a0, ys + a1);
        if a0 >= r && a1 >= ro {
            ms.course(x, y, z, (0.7, 1.4), sb(0.03).skip(NEG_Y));
        } else {
            let xb = (x_in(a0) + x_out(a1)) * 0.5;
            if xc - xb > x.0 + 0.05 {
                ms.course((x.0, xc - xb), y, z, (0.6, 1.3), sb(0.03).skip(NEG_Y));
            }
            if xc + xb < x.1 - 0.05 {
                ms.course((xc + xb, x.1), y, z, (0.6, 1.3), sb(0.03).skip(NEG_Y));
            }
        }
    }
}

/// One 10° sector of the stands (see the module docs for the frame): curb and drain gutter,
/// the podium wall (matching the first-tier collider), five rows of seats, half of a radial
/// stair at each edge, the top walkway, the outer wall reaching 2 m into the ground and an
/// arcade of two arches with an attic and cornice.
pub fn stand_segment(seed: u64) -> Mesh {
    let mut low = Mason::new(seed, Tone::Sand);
    let zr = 41.0;
    let xw = zr * T5;
    curb(&mut low, xw);
    // Gutter with a slotted drain cover in the middle.
    low.tone = Tone::Grime;
    for x in [(-xw, -0.32), (0.32, xw)] {
        low.course(
            x,
            (-0.14, -0.005),
            (40.75, 40.99),
            (1.0, 1.6),
            sb(0.02).skip(NEG_Y),
        );
    }
    low.block(
        Vec3::new(-0.32, -0.45, 40.75),
        Vec3::new(0.32, -0.08, 40.99),
        sb(0.01),
    );
    low.tone = Tone::Sand;
    for k in 0..5 {
        let x = -0.25 + 0.125 * k as f32;
        low.block(
            Vec3::new(x - 0.035, -0.08, 40.75),
            Vec3::new(x + 0.035, -0.005, 40.99),
            sb(0.01),
        );
    }
    // Podium: plinth course, orthostats and a coping that overhangs 4 cm.
    low.course(
        (-xw, xw),
        (-0.1, 0.18),
        (40.97, 41.4),
        (1.2, 1.9),
        sb(0.03).skip(NEG_Y),
    );
    low.course(
        (-xw, xw),
        (0.18, 0.645),
        (41.0, 41.4),
        (0.8, 1.5),
        sb(0.03).skip(NEG_Y),
    );
    low.lid = NEG_Y;
    low.course(
        (-xw, xw),
        (0.64, 0.8),
        (40.96, 42.72),
        (1.3, 2.0),
        sb(0.05).skip(NEG_Y),
    );
    low.lid = 0;
    // Seats: a slab with a rounded nosing over a recessed riser, five rows.
    let sx = (-xw + 0.45, xw - 0.45);
    for j in 0..5 {
        let zf = 42.6 + 0.8 * j as f32;
        let top = 0.8 + 0.4 * (j + 1) as f32;
        low.course(
            sx,
            (top - 0.4, top - 0.12),
            (zf + 0.05, zf + 0.82),
            (1.2, 2.0),
            sb(0.02).skip(NEG_Y | POS_Z),
        );
        low.lid = NEG_Y;
        low.course(
            sx,
            (top - 0.12, top),
            (zf, zf + 0.92),
            (1.3, 2.2),
            sb(0.04).skip(NEG_Y | POS_Z),
        );
        low.lid = 0;
    }
    // Walkway at the top.
    low.course(
        (-xw, xw),
        (2.8, 3.08),
        (46.65, 47.22),
        (0.9, 1.5),
        sb(0.02).skip(NEG_Y),
    );
    low.lid = NEG_Y;
    low.course(
        (-xw, xw),
        (3.08, 3.2),
        (46.6, 47.22),
        (1.1, 1.8),
        sb(0.04).skip(NEG_Y),
    );
    low.lid = 0;
    // Half a stair at each edge (the neighbor sector holds the other half).
    for s in [-1.0f32, 1.0] {
        let (x0, x1) = if s > 0.0 {
            (xw - 0.45, xw)
        } else {
            (-xw, -xw + 0.45)
        };
        for k in 0..10 {
            let z0 = 42.6 + 0.4 * k as f32;
            let top = 0.8 + 0.2 * (k + 1) as f32;
            low.block(
                Vec3::new(x0, top - 0.25, z0),
                Vec3::new(x1, top, z0 + 0.43),
                sb(0.03).skip(NEG_Y),
            );
        }
    }
    let mut lower = low.mesh;
    shear(&mut lower, zr);

    let mut up = Mason::new(seed ^ 0xabc, Tone::Sand);
    let zr = 47.5;
    let xa = zr * T5;
    let z = (47.2, 47.8);
    // Outer wall from 2 m below ground to the walkway, then a string course.
    up.tone = Tone::Grime;
    up.course(
        (-xa, xa),
        (-2.0, -0.6),
        z,
        (1.4, 2.4),
        sb(0.03).skip(NEG_Y | NEG_Z),
    );
    up.tone = Tone::Sand;
    let mut y = -0.6;
    for top in [0.15, 0.85, 1.55, 2.3, 3.1] {
        up.course(
            (-xa, xa),
            (y, top),
            z,
            (1.2, 2.2),
            sb(0.03).skip(NEG_Y | NEG_Z),
        );
        y = top;
    }
    up.lid = NEG_Y;
    up.course(
        (-xa, xa),
        (3.1, 3.3),
        (47.2, 47.84),
        (1.4, 2.2),
        sb(0.04).skip(NEG_Y),
    );
    up.lid = 0;
    // Arcade: piers, imposts, two arches, attic and cornice.
    let piers = [(-xa, -xa + 0.55), (-0.55, 0.55), (xa - 0.55, xa)];
    for &(a, b) in &piers {
        let mut y = 3.3;
        for top in [4.05, 4.8, 5.47] {
            up.block(
                Vec3::new(a, y, z.0),
                Vec3::new(b, top, z.1),
                sb(0.03).skip(NEG_Y),
            );
            y = top;
        }
        up.lid = NEG_Y;
        up.block(
            Vec3::new(a - 0.035, 5.47, z.0 - 0.035),
            Vec3::new(b + 0.035, 5.6, z.1 + 0.035),
            sb(0.025).skip(NEG_Y),
        );
        up.lid = 0;
    }
    let r = xa * 0.5 - 0.55;
    for xc in [-xa * 0.5, xa * 0.5] {
        let bay = if xc < 0.0 { (-xa, 0.0) } else { (0.0, xa) };
        arch(
            &mut up,
            xc,
            5.6,
            r,
            r + 0.42,
            9,
            z,
            bay,
            &[0.0, 0.5, 1.0, 1.55, 2.0],
        );
    }
    up.course((-xa, xa), (7.6, 8.2), z, (1.2, 2.2), sb(0.03).skip(NEG_Y));
    up.lid = NEG_Y;
    up.course(
        (-xa, xa),
        (8.2, 8.3),
        (47.17, 47.83),
        (1.6, 2.4),
        sb(0.03).skip(NEG_Y),
    );
    up.course(
        (-xa, xa),
        (8.3, 8.48),
        (47.13, 47.85),
        (1.6, 2.4),
        sb(0.05).skip(NEG_Y),
    );
    up.lid = 0;
    let mut upper = up.mesh;
    shear(&mut upper, zr);
    lower.merge(&upper);
    lower
}

/// The gate sector: curb threshold, paved passage on a foundation, two flank walls with
/// raking copings (each half in the neighbor sector, hiding the cut ends of the seats), and
/// the gate: a big arch with a raised portcullis, an attic with a tablet and two heavy doors
/// standing open. Returns (stone, wood, iron).
pub fn entrance_meshes(seed: u64) -> (Mesh, Mesh, Mesh) {
    let mut low = Mason::new(seed, Tone::Sand);
    let zr = 41.0;
    let xw = zr * T5;
    curb(&mut low, xw);
    // Paving.
    low.wobble = 0.006;
    let mut zc = 40.75;
    while zc < 47.79 {
        let depth = low.rng.range(0.7, 1.1).min(47.8 - zc);
        let depth = if 47.8 - zc - depth < 0.4 {
            47.8 - zc
        } else {
            depth
        };
        for (a, b) in run(&mut low.rng, -xw, xw, 0.7, 1.3) {
            let top = 0.035 + low.rng.signed() * 0.006;
            low.block(
                Vec3::new(a, -0.25, zc),
                Vec3::new(b, top, zc + depth),
                sb(0.03).skip(NEG_Y),
            );
        }
        zc += depth;
    }
    low.wobble = 0.003;
    low.tone = Tone::Grime;
    low.block(
        Vec3::new(-xw, -1.7, 44.6),
        Vec3::new(xw, -0.25, 47.8),
        sb(0.05),
    );
    low.tone = Tone::Sand;
    // Flank walls on the sector edges.
    let slope = (3.95 - 1.45) / (47.2 - 41.6);
    let top = |z: f32| 1.45 + (z - 41.6) * slope;
    for s in [-1.0f32, 1.0] {
        let xc = s * xw;
        let (x0, x1) = (xc - 0.45, xc + 0.45);
        // Front pier with a cap.
        let mut y = -0.3;
        for t in [0.5, 1.15, 1.72] {
            low.block(
                Vec3::new(x0 - 0.04, y, 41.0),
                Vec3::new(x1 + 0.04, t, 41.6),
                sb(0.03).skip(NEG_Y),
            );
            y = t;
        }
        low.lid = NEG_Y;
        low.block(
            Vec3::new(x0 - 0.08, 1.72, 40.96),
            Vec3::new(x1 + 0.08, 1.9, 41.64),
            sb(0.05).skip(NEG_Y),
        );
        low.lid = 0;
        // Body in courses; the top course follows the slope under the coping.
        let mut yb = -1.7;
        for yt in [-0.6, 0.0, 0.55, 1.1, 1.65, 2.2, 2.75, 3.3, 3.85] {
            for (za, zb) in run(&mut low.rng, 41.6, 47.2, 0.9, 1.6) {
                let lim_a = top(za) - 0.2;
                let lim_b = top(zb) - 0.2;
                if lim_b <= yb + 0.06 {
                    continue;
                }
                if yt <= lim_a {
                    low.block(
                        Vec3::new(x0, yb, za),
                        Vec3::new(x1, yt, zb),
                        sb(0.03).skip(NEG_Y),
                    );
                } else {
                    // Clip the block's top to the slope.
                    let za = if lim_a < yb + 0.06 {
                        41.6 + (yb + 0.26 - 1.45) / slope
                    } else {
                        za
                    };
                    if zb - za < 0.15 {
                        continue;
                    }
                    let h = (yt - yb) * 0.5;
                    let mut b =
                        rounded_box(Vec3::new(0.45, h, (zb - za) * 0.5), sb(0.03).skip(NEG_Y));
                    let (zm, ym) = ((za + zb) * 0.5, (yb + yt) * 0.5);
                    deform(&mut b, |p| {
                        let zz = p.z + zm;
                        let t = (p.y + h) / (2.0 * h);
                        let lim = (top(zz) - 0.2).min(yt);
                        Vec3::new(p.x + xc, yb + t * (lim - yb), zz)
                    });
                    let _ = ym;
                    low.add(b, Mat4::IDENTITY);
                }
            }
            yb = yt;
        }
        // Raking coping.
        let ang = slope.atan();
        for (za, zb) in run(&mut low.rng, 41.6, 47.2, 1.0, 1.6) {
            let zm = (za + zb) * 0.5;
            let len = (zb - za) / ang.cos();
            let b = rounded_box(Vec3::new(0.5, 0.11, len * 0.5 + 0.004), sb(0.04));
            let rot = Quat::from_rotation_x(-ang);
            let center = Vec3::new(xc, top(zm) - 0.09, zm);
            low.add(b, Mat4::from_rotation_translation(rot, center));
        }
    }
    let mut stone = low.mesh;
    shear(&mut stone, zr);

    // The gate in the arcade line.
    let mut up = Mason::new(seed ^ 0x9a7e, Tone::Sand);
    let zr = 47.5;
    let xa = zr * T5;
    let z = (47.2, 47.8);
    let r = xa - 0.6;
    for s in [-1.0f32, 1.0] {
        let (a, b) = if s > 0.0 {
            (xa - 0.6, xa)
        } else {
            (-xa, -xa + 0.6)
        };
        up.tone = Tone::Grime;
        up.block(Vec3::new(a, -2.0, z.0), Vec3::new(b, -0.6, z.1), sb(0.03));
        up.tone = Tone::Sand;
        let mut y = -0.6;
        for t in [0.4, 1.4, 2.4, 3.45] {
            up.block(
                Vec3::new(a, y, z.0),
                Vec3::new(b, t, z.1),
                sb(0.035).skip(NEG_Y),
            );
            y = t;
        }
        up.lid = NEG_Y;
        up.block(
            Vec3::new(a - 0.04, 3.45, z.0 - 0.04),
            Vec3::new(b + 0.04, 3.6, z.1 + 0.04),
            sb(0.025).skip(NEG_Y),
        );
        up.lid = 0;
    }
    arch(
        &mut up,
        0.0,
        3.6,
        r,
        r + 0.6,
        13,
        z,
        (-xa, xa),
        &[0.0, 0.6, 1.2, 1.8, 2.4, 3.0, 3.58, 4.16, 4.6],
    );
    up.lid = NEG_Y;
    up.course(
        (-xa, xa),
        (8.2, 8.3),
        (47.17, 47.83),
        (1.6, 2.4),
        sb(0.03).skip(NEG_Y),
    );
    up.course(
        (-xa, xa),
        (8.3, 8.48),
        (47.13, 47.85),
        (1.6, 2.4),
        sb(0.05).skip(NEG_Y),
    );
    up.lid = 0;
    // Raised attic with a framed tablet facing the arena.
    let ax = (-xa + 0.35, xa - 0.35);
    up.course(ax, (8.48, 9.1), z, (1.3, 2.2), sb(0.03).skip(NEG_Y));
    up.course(ax, (9.1, 9.72), z, (1.3, 2.2), sb(0.03).skip(NEG_Y));
    up.lid = NEG_Y;
    up.course(
        (ax.0 - 0.06, ax.1 + 0.06),
        (9.72, 9.95),
        (47.12, 47.88),
        (1.6, 2.6),
        sb(0.05).skip(NEG_Y),
    );
    up.lid = 0;
    up.tone = Tone::Lime;
    let (tx, ty) = (1.7, (8.62, 9.6));
    up.block(
        Vec3::new(-tx, ty.0, 47.16),
        Vec3::new(tx, ty.0 + 0.1, 47.22),
        sb(0.02),
    );
    up.block(
        Vec3::new(-tx, ty.1 - 0.1, 47.16),
        Vec3::new(tx, ty.1, 47.22),
        sb(0.02),
    );
    up.block(
        Vec3::new(-tx, ty.0, 47.16),
        Vec3::new(-tx + 0.1, ty.1, 47.22),
        sb(0.02),
    );
    up.block(
        Vec3::new(tx - 0.1, ty.0, 47.16),
        Vec3::new(tx, ty.1, 47.22),
        sb(0.02),
    );
    let mut gate = up.mesh;
    shear(&mut gate, zr);
    stone.merge(&gate);

    // Portcullis, raised: its spiked bottom hangs in the arch.
    let mut iron = Mesh::default();
    let bar = |m: &mut Mesh, min: Vec3, max: Vec3| {
        let b = rounded_box((max - min) * 0.5, BoxOpts::round(0.008));
        append(m, &b, Mat4::from_translation((min + max) * 0.5));
    };
    let pz = (47.47, 47.53);
    let mut x = -3.45;
    while x <= 3.46 {
        bar(
            &mut iron,
            Vec3::new(x - 0.028, 5.0, pz.0),
            Vec3::new(x + 0.028, 8.0, pz.1),
        );
        let spike = lathe(
            &[&[
                Vec2::new(0.0, 4.78),
                Vec2::new(0.045, 5.0),
                Vec2::new(0.0, 5.0),
            ]],
            4,
            0.05,
            &|_, _, r| r,
        );
        append(
            &mut iron,
            &spike,
            Mat4::from_translation(Vec3::new(x, 0.0, 47.5)) * Mat4::from_rotation_y(FRAC_PI_4_F),
        );
        x += 0.3;
    }
    for y in [5.25, 5.85, 6.45, 7.05, 7.65] {
        bar(
            &mut iron,
            Vec3::new(-3.5, y - 0.03, pz.0 - 0.012),
            Vec3::new(3.5, y + 0.03, pz.1 + 0.012),
        );
    }
    // Two doors standing open against the flank walls.
    let mut wood = Mesh::default();
    let mut rng = Rng::new(seed ^ 0xd00);
    for s in [-1.0f32, 1.0] {
        let mut leaf_wood = Mesh::default();
        let mut leaf_iron = Mesh::default();
        let width = 3.45;
        // Built closed, hinge at x = 0, leaf toward −x (then mirrored for the left door).
        let mut x0 = 0.0;
        while x0 < width - 0.01 {
            let w = (0.26f32).min(width - x0);
            let xm = x0 + w * 0.5;
            let arch_x = r - 0.06 - xm; // distance from the arch center
            let h = 3.6 + (r - 0.08).powi(2).sub_sqrt(arch_x) - 0.06;
            let plank = rounded_box(
                Vec3::new(0.5 * w - 0.004, (h - 0.04) * 0.5, 0.055),
                BoxOpts::round(0.012).bevel(2),
            );
            let mut p = transformed(
                &plank,
                Mat4::from_translation(Vec3::new(-xm, 0.04 + (h - 0.04) * 0.5, 0.0)),
            );
            tint(
                &mut p,
                Vec4::new(
                    rng.range(0.8, 1.0),
                    rng.range(0.78, 0.95),
                    rng.range(0.72, 0.9),
                    1.0,
                ),
            );
            offset_uv(&mut p, Vec2::new(rng.range(0.0, 8.0), rng.range(0.0, 8.0)));
            leaf_wood.merge(&p);
            x0 += w;
        }
        // Battens on the back, a brace, straps and studs on the front.
        for y in [0.5, 2.0, 3.4] {
            let b = rounded_box(
                Vec3::new(width * 0.5 - 0.1, 0.09, 0.035),
                BoxOpts::round(0.012).bevel(2),
            );
            leaf_wood.merge(&transformed(
                &b,
                Mat4::from_translation(Vec3::new(-width * 0.5, y, 0.085)),
            ));
        }
        let brace_len = ((width - 0.3).powi(2) + 1.5f32.powi(2)).sqrt();
        let brace = rounded_box(
            Vec3::new(brace_len * 0.5, 0.08, 0.03),
            BoxOpts::round(0.012).bevel(2),
        );
        leaf_wood.merge(&transformed(
            &brace,
            Mat4::from_rotation_translation(
                Quat::from_rotation_z((1.5f32).atan2(width - 0.3)),
                Vec3::new(-width * 0.5, 1.25, 0.08),
            ),
        ));
        for y in [0.6, 1.9, 3.2] {
            let strap = rounded_box(Vec3::new(width * 0.42, 0.045, 0.006), BoxOpts::round(0.004));
            leaf_iron.merge(&transformed(
                &strap,
                Mat4::from_translation(Vec3::new(-width * 0.42, y, -0.061)),
            ));
            let mut sx = 0.12;
            while sx < width * 0.84 {
                let stud = stud_mesh(0.018);
                leaf_iron.merge(&transformed(
                    &stud,
                    Mat4::from_rotation_translation(
                        Quat::from_rotation_x(-FRAC_PI_2),
                        Vec3::new(-sx, y, -0.066),
                    ),
                ));
                sx += 0.3;
            }
        }
        // Open the leaf toward the arena and put it on its hinge.
        let open = Mat4::from_translation(Vec3::new(s * (r - 0.04), 0.0, 47.1))
            * Mat4::from_rotation_y(s * -1.48)
            * Mat4::from_scale(Vec3::new(-s, 1.0, 1.0));
        append(&mut wood, &leaf_wood, open);
        append(&mut iron, &leaf_iron, open);
    }
    (stone, wood, iron)
}

const FRAC_PI_4_F: f32 = std::f32::consts::FRAC_PI_4;

trait SubSqrt {
    fn sub_sqrt(self, x: f32) -> f32;
}

impl SubSqrt for f32 {
    /// `sqrt(self - x²)`, or 0 when negative (height of a circle of squared radius `self`).
    fn sub_sqrt(self, x: f32) -> f32 {
        (self - x * x).max(0.0).sqrt()
    }
}

/// A round nail or rivet head facing +Y.
pub fn stud_mesh(r: f32) -> Mesh {
    lathe(
        &[&[
            Vec2::new(r, -0.002),
            Vec2::new(r, 0.0),
            Vec2::new(r * 0.85, r * 0.45),
            Vec2::new(r * 0.45, r * 0.7),
            Vec2::new(0.0, r * 0.75),
        ]],
        6,
        r,
        &|_, _, r| r,
    )
}

// ============================================================================== columns

/// A Doric column for the 7 m pillar colliders (mesh origin at the collider's center, 3.5 m
/// up), inside the collider where fighters can touch it: a round plinth with a chipped edge,
/// an attic base (torus, scotia, torus), a fluted shaft of five drums with entasis and chipped
/// arrises, astragal, echinus and a chipped square abacus. `variant` 1 is more battered.
pub fn pillar_mesh(variant: u32) -> Mesh {
    let seed = 700 + variant as u64 * 31;
    let s32 = seed as u32;
    let mut m = Mesh::default();
    let mut rng = Rng::new(seed);
    let lime = |rng: &mut Rng| tone(rng, Tone::Lime);
    let wear = move |a: f32, y: f32, r: f32| {
        let p = Vec3::new(a.cos() * r, y, a.sin() * r);
        r - 0.012 * smooth(0.2, 0.8, vn3(p * 6.0, s32)) - 0.004 * vn3(p * 20.0, 5)
    };
    // Round plinth (inside the collider), its top edge chipped; variant 1 is more battered.
    let battered = if variant == 1 { 1.6 } else { 1.0 };
    let chipped = move |a: f32, y: f32, r: f32| {
        let p = Vec3::new(a.cos() * r, y, a.sin() * r);
        let edge = smooth(0.12, 0.3, y);
        let n = vn3(p * 7.0, s32 ^ 0x51) * 0.7 + vn3(p * 19.0, s32 ^ 0x52) * 0.3;
        r - 0.045 * battered * edge * smooth(0.15, 0.6, n) - 0.003 * vn3(p * 30.0, s32)
    };
    let plinth = [
        Vec2::new(1.14, -0.05),
        Vec2::new(1.14, 0.12),
        Vec2::new(1.14, 0.26),
        Vec2::new(1.125, 0.29),
        Vec2::new(1.1, 0.3),
        Vec2::new(1.0, 0.3),
    ];
    let mut p = lathe(&[&plinth], 72, 1.1, &chipped);
    tint(&mut p, lime(&mut rng));
    m.merge(&p);
    // Attic base: torus, scotia, torus, fillet.
    let mut base: Vec<Vec2> = Vec::new();
    base.extend(arc_points(
        Vec2::new(1.06, 0.375),
        0.075,
        -FRAC_PI_2,
        FRAC_PI_2,
        8,
    ));
    base.extend([
        Vec2::new(1.04, 0.47),
        Vec2::new(1.025, 0.5),
        Vec2::new(1.035, 0.53),
        Vec2::new(1.05, 0.545),
    ]);
    base.extend(arc_points(
        Vec2::new(1.05, 0.59),
        0.045,
        -FRAC_PI_2,
        FRAC_PI_2,
        6,
    ));
    base.extend([Vec2::new(1.07, 0.66), Vec2::new(1.07, 0.69)]);
    let mut b = lathe(&[&base], 64, 1.1, &wear);
    tint(&mut b, lime(&mut rng));
    m.merge(&b);
    // Shaft: five drums with entasis, twenty flutes with chipped arrises.
    let (y0, y1) = (0.69, 6.08);
    let mut ys: Vec<f32> = steps(y0, y1, 22);
    let joints: Vec<f32> = (1..5).map(|k| y0 + (y1 - y0) * k as f32 / 5.0).collect();
    for &j in &joints {
        ys.extend([j - 0.008, j, j + 0.008]);
    }
    ys.extend([y0 + 0.03, y1 - 0.03]);
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    ys.dedup_by(|a, b| (*a - *b).abs() < 0.003);
    let radius_at = |y: f32| {
        let t = (y - y0) / (y1 - y0);
        1.06 - 0.14 * t.powf(1.5) + 0.012 * (t * PI).sin()
    };
    let shaft: Vec<Vec2> = ys.iter().map(|&y| Vec2::new(radius_at(y), y)).collect();
    let flutes = 20.0;
    let jseed = s32 ^ 0x1234;
    let fluted = move |a: f32, y: f32, r: f32| {
        let phi = (a / TAU * flutes).rem_euclid(1.0) - 0.5;
        let groove = (1.0 - (phi / 0.44).powi(2)).max(0.0).sqrt();
        let ends = smooth(y0, y0 + 0.06, y) * (1.0 - smooth(y1 - 0.06, y1, y));
        let mut r = r - 0.034 * groove * ends;
        for &j in &joints {
            r -= 0.006 * (1.0 - smooth(0.0, 0.008, (y - j).abs()));
        }
        let p = Vec3::new(a.cos(), y, a.sin());
        let arris = smooth(0.38, 0.5, phi.abs());
        let n = vn3(Vec3::new(p.x * 9.0, y * 7.0, p.z * 9.0), jseed);
        r -= 0.03 * arris * smooth(0.25, 0.7, n);
        r -= 0.004 * vn3(Vec3::new(p.x * 25.0, y * 18.0, p.z * 25.0), jseed ^ 9);
        r
    };
    let mut s = lathe(&[&shaft], 120, 1.0, &fluted);
    // Rain streaks from the capital and dust at the foot.
    let sseed = s32 ^ 0x77;
    paint(&mut s, |p, _| {
        let a = p.z.atan2(p.x);
        let streak = smooth(0.35, 0.8, vn3(Vec3::new(a * 6.0, p.y * 0.25, 0.0), sseed));
        let fall = smooth(1.5, 6.0, p.y);
        let k = 1.0 - 0.22 * streak * fall;
        Vec4::new(k, k * 0.98, k * 0.95, 1.0)
    });
    tint(&mut s, lime(&mut rng));
    m.merge(&s);
    // Astragal and echinus.
    let mut cap: Vec<Vec2> = vec![Vec2::new(radius_at(y1) - 0.02, y1 - 0.005)];
    cap.extend(arc_points(
        Vec2::new(0.925, y1 + 0.035),
        0.035,
        -FRAC_PI_2,
        FRAC_PI_2,
        6,
    ));
    cap.push(Vec2::new(0.93, y1 + 0.09));
    for k in 0..=8 {
        let t = k as f32 / 8.0;
        let r = 0.94 + 0.19 * (t * FRAC_PI_2).sin().powf(0.7);
        cap.push(Vec2::new(r, y1 + 0.09 + 0.33 * t));
    }
    let top = y1 + 0.42;
    cap.extend([Vec2::new(1.13, top), Vec2::new(0.8, top)]);
    let mut c = lathe(&[&cap], 64, 1.0, &wear);
    tint(&mut c, lime(&mut rng));
    m.merge(&c);
    // Square abacus, its top at the collider's top (7 m).
    let ah = (7.0 - top) * 0.5;
    let mut abacus = chipped_block(
        Vec3::new(1.08, ah, 1.08),
        Chip {
            voxel: 0.065,
            depth: 0.05,
            breaks: if variant == 1 { 4 } else { 2 },
            open_bottom: false,
            ..Chip::new(seed ^ 0xabac)
        },
    );
    tint(&mut abacus, lime(&mut rng));
    append(
        &mut m,
        &abacus,
        Mat4::from_translation(Vec3::new(0.0, top + ah, 0.0)),
    );
    grime(&mut m, 0.0, 0.9, 0.35);
    offset_uv(&mut m, Vec2::new(rng.range(0.0, 4.0), rng.range(0.0, 4.0)));
    transformed(&m, Mat4::from_translation(Vec3::new(0.0, -3.5, 0.0)))
}

// ============================================================================== cover

/// A low wall of chipped ashlar for the 3.2 × 1.1 × 0.5 m cover colliders (origin at the
/// collider's center): two courses over a recessed mortar core and a coping.
pub fn stone_wall_mesh(seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let mut m = Mesh::default();
    let mut core = rounded_box(Vec3::new(1.575, 0.5, 0.215), BoxOpts::round(0.02));
    tint(&mut core, tone(&mut rng, Tone::Mortar));
    append(
        &mut m,
        &core,
        Mat4::from_translation(Vec3::new(0.0, -0.05, 0.0)),
    );
    let courses = [(-0.6, -0.07), (-0.07, 0.4)];
    for (i, &(y0, y1)) in courses.iter().enumerate() {
        for (x0, x1) in run(&mut rng, -1.6, 1.6, 0.75, 1.25) {
            let half = Vec3::new(
                (x1 - x0) * 0.5 - 0.005,
                (y1 - y0) * 0.5 - 0.005,
                0.25 - rng.range(0.0, 0.012),
            );
            let mut b = chipped_block(
                half,
                Chip {
                    voxel: 0.04,
                    depth: 0.045,
                    breaks: 2,
                    open_bottom: i == 0,
                    ..Chip::new(rng.0)
                },
            );
            rng.f();
            tint(&mut b, tone(&mut rng, Tone::Sand));
            offset_uv(&mut b, Vec2::new(rng.range(0.0, 8.0), rng.range(0.0, 8.0)));
            let rot = Quat::from_euler(
                EulerRot::YXZ,
                rng.signed() * 0.01,
                rng.signed() * 0.005,
                rng.signed() * 0.006,
            );
            let center = Vec3::new((x0 + x1) * 0.5, (y0 + y1) * 0.5, rng.signed() * 0.008);
            append(&mut m, &b, Mat4::from_rotation_translation(rot, center));
        }
    }
    for (x0, x1) in run(&mut rng, -1.6, 1.6, 1.1, 1.7) {
        let half = Vec3::new((x1 - x0) * 0.5 - 0.005, 0.075, 0.27);
        let mut b = chipped_block(
            half,
            Chip {
                radius: 0.03,
                voxel: 0.04,
                depth: 0.04,
                breaks: 2,
                open_bottom: true,
                ..Chip::new(rng.0 ^ 0xc0)
            },
        );
        rng.f();
        tint(&mut b, tone(&mut rng, Tone::Sand));
        offset_uv(&mut b, Vec2::new(rng.range(0.0, 8.0), rng.range(0.0, 8.0)));
        append(
            &mut m,
            &b,
            Mat4::from_translation(Vec3::new((x0 + x1) * 0.5, 0.475, 0.0)),
        );
    }
    grime(&mut m, -0.55, 0.45, 0.4);
    m
}

/// Rubble at the foot of a low wall (the frame of [`stone_wall_mesh`]; the ground is at
/// y = −0.55): fallen fragments and chips heaped against the +Z face toward one end (+X for
/// variant 0, −X for 1), smaller ones resting on the bigger, grit scattered further out and a
/// fragment by that end.
pub fn rubble_mesh(variant: u32) -> Mesh {
    let mut rng = Rng::new(0x5ab1e + variant as u64 * 31);
    let side = if variant.is_multiple_of(2) { 1.0 } else { -1.0 };
    let ground = -0.55;
    let mut m = Mesh::default();
    // Fragments placed so far: center (x, z), footprint radius, top.
    let mut placed: Vec<(Vec2, f32, f32)> = Vec::new();
    // A tumbled fragment at `x`, `z_out` in front of the wall face, resting on the sand (a
    // little sunk in) or on the fragments below it.
    let mut drop = |m: &mut Mesh, rng: &mut Rng, x: f32, z_out: f32, half: Vec3, voxel: f32| {
        let rot = Quat::from_euler(
            EulerRot::YXZ,
            rng.range(0.0, TAU),
            rng.signed() * 0.5,
            rng.signed() * 0.5,
        );
        let mut b = chipped_block(
            half,
            Chip {
                radius: 0.008,
                voxel,
                depth: half.min_element() * 0.6,
                breaks: 6,
                open_bottom: false,
                ..Chip::new(rng.0)
            },
        );
        rng.f();
        tint(&mut b, tone(rng, Tone::Sand));
        offset_uv(&mut b, Vec2::new(rng.range(0.0, 8.0), rng.range(0.0, 8.0)));
        let r = Mat3::from_quat(rot);
        let e = Vec3::new(
            r.row(0).abs().dot(half),
            r.row(1).abs().dot(half),
            r.row(2).abs().dot(half),
        );
        let at = Vec2::new(x, 0.26 + e.z + z_out);
        let rho = e.x.max(e.z);
        let base = placed
            .iter()
            .filter(|(c, rc, _)| c.distance(at) < (rc + rho) * 0.55)
            .map(|p| p.2)
            .fold(ground, f32::max);
        let y = base + e.y * 0.8;
        placed.push((at, rho, y + e.y * 0.75));
        append(
            m,
            &b,
            Mat4::from_rotation_translation(rot, Vec3::new(at.x, y, at.y)),
        );
    };
    let xc = side * 0.85;
    for _ in 0..4 {
        let half = Vec3::new(
            rng.range(0.1, 0.17),
            rng.range(0.06, 0.09),
            rng.range(0.08, 0.12),
        );
        let (x, z) = (xc + rng.signed() * 0.5, rng.range(0.0, 0.15));
        drop(&mut m, &mut rng, x, z, half, 0.03);
    }
    for _ in 0..7 {
        let half = Vec3::new(
            rng.range(0.045, 0.075),
            rng.range(0.03, 0.05),
            rng.range(0.035, 0.06),
        );
        let (x, z) = (xc + rng.signed() * 0.6, rng.range(0.0, 0.3));
        drop(&mut m, &mut rng, x, z, half, 0.024);
    }
    for _ in 0..10 {
        let half = Vec3::new(
            rng.range(0.02, 0.04),
            rng.range(0.014, 0.028),
            rng.range(0.016, 0.035),
        );
        let (x, z) = (xc + rng.signed() * 0.85, rng.range(0.0, 0.55));
        drop(&mut m, &mut rng, x, z, half, 0.016);
    }
    let half = Vec3::new(0.08, 0.05, 0.07);
    let z = rng.range(-0.45, -0.3);
    drop(&mut m, &mut rng, side * 1.78, z, half, 0.024);
    // Grit: tiny sharp chips (too small to need rounding).
    for _ in 0..20 {
        let half = Vec3::new(
            rng.range(0.006, 0.014),
            rng.range(0.004, 0.009),
            rng.range(0.005, 0.012),
        );
        let mut b = rounded_box(half, BoxOpts::round(0.0));
        tint(&mut b, tone(&mut rng, Tone::Sand));
        offset_uv(&mut b, Vec2::new(rng.range(0.0, 8.0), rng.range(0.0, 8.0)));
        let rot = Quat::from_euler(EulerRot::YXZ, rng.range(0.0, TAU), rng.signed(), 0.0);
        let at = Vec3::new(
            xc + rng.signed() * 1.0,
            ground + half.y * 0.5,
            0.3 + rng.range(0.0, 0.8),
        );
        append(&mut m, &b, Mat4::from_rotation_translation(rot, at));
    }
    grime(&mut m, ground, 0.1, 0.3);
    // Sand dust settled on top.
    paint(&mut m, |_, n| {
        let k = smooth(0.4, 1.0, n.y) * 0.1;
        Vec4::new(1.0 + k, 1.0 + k * 0.85, 1.0 + k * 0.6, 1.0)
    });
    m
}

/// A plank (grain along its length, local X) of `size` with soft edges, random tint and
/// texture offset.
fn plank(rng: &mut Rng, size: Vec3, round: f32) -> Mesh {
    let mut p = rounded_box(size * 0.5, BoxOpts::round(round));
    let v = rng.range(0.78, 1.0);
    tint(
        &mut p,
        Vec4::new(v, v * rng.range(0.94, 1.0), v * rng.range(0.88, 1.0), 1.0),
    );
    offset_uv(&mut p, Vec2::new(rng.range(0.0, 8.0), rng.range(0.0, 8.0)));
    p
}

/// A plank standing upright (grain along Y).
fn post(rng: &mut Rng, size: Vec3, round: f32) -> Mesh {
    let p = plank(rng, Vec3::new(size.y, size.x, size.z), round);
    transformed(&p, Mat4::from_rotation_z(FRAC_PI_2))
}

fn put(dst: &mut Mesh, src: &Mesh, at: Vec3) {
    append(dst, src, Mat4::from_translation(at));
}

/// A timber barricade for the cover colliders: posts, planked faces on both sides, a cap
/// rail, iron straps over the top and bolts. Returns (wood, iron).
pub fn timber_wall_meshes(seed: u64) -> (Mesh, Mesh) {
    let mut rng = Rng::new(seed);
    let mut wood = Mesh::default();
    let mut iron = Mesh::default();
    for x in [-1.48f32, 0.0, 1.48] {
        let w = if x == 0.0 { 0.2 } else { 0.22 };
        let p = post(&mut rng, Vec3::new(w, 1.08, 0.36), 0.025);
        put(&mut wood, &p, Vec3::new(x, -0.02, 0.0));
    }
    for face in [-1.0f32, 1.0] {
        for k in 0..4 {
            let y = -0.53 + 0.265 * k as f32 + 0.125;
            let len = 2.98 + rng.signed() * 0.04;
            let p = plank(&mut rng, Vec3::new(len, 0.25, 0.07), 0.015);
            let rot = Quat::from_rotation_z(rng.signed() * 0.01);
            append(
                &mut wood,
                &p,
                Mat4::from_rotation_translation(
                    rot,
                    Vec3::new(rng.signed() * 0.03, y, face * 0.205),
                ),
            );
        }
    }
    let cap = plank(&mut rng, Vec3::new(3.2, 0.07, 0.5), 0.02);
    put(&mut wood, &cap, Vec3::new(0.0, 0.515, 0.0));
    for x in [-0.75f32, 0.75] {
        for face in [-1.0f32, 1.0] {
            let s = rounded_box(Vec3::new(0.035, 0.5, 0.004), BoxOpts::round(0.003));
            put(&mut iron, &s, Vec3::new(x, 0.0, face * 0.2445));
            for k in 0..4 {
                let stud = stud_mesh(0.016);
                append(
                    &mut iron,
                    &stud,
                    Mat4::from_rotation_translation(
                        Quat::from_rotation_x(face * FRAC_PI_2),
                        Vec3::new(x, -0.4 + 0.265 * k as f32, face * 0.248),
                    ),
                );
            }
        }
        let s = rounded_box(Vec3::new(0.035, 0.004, 0.252), BoxOpts::round(0.003));
        put(&mut iron, &s, Vec3::new(x, 0.552, 0.0));
    }
    grime(&mut wood, -0.55, 0.35, 0.35);
    (wood, iron)
}

/// One crate of `size`: framed faces of planks with a diagonal brace, a planked lid, iron
/// corner plates and nails; origin at its center. Returns (wood, iron).
fn crate_box(rng: &mut Rng, size: Vec3) -> (Mesh, Mesh) {
    let mut wood = Mesh::default();
    let mut iron = Mesh::default();
    let h = size * 0.5;
    // Four sides: rotate one face layout around Y.
    for k in 0..4 {
        let rot = Quat::from_rotation_y(k as f32 * FRAC_PI_2);
        let (w, d) = if k % 2 == 0 {
            (size.x, h.z)
        } else {
            (size.z, h.x)
        };
        let mut face = Mesh::default();
        let mut fi = Mesh::default();
        // Panel planks, set back from the frame.
        let n = 4;
        let ph = (size.y - 0.04) / n as f32;
        for j in 0..n {
            let y = -h.y + 0.02 + ph * (j as f32 + 0.5);
            let p = plank(rng, Vec3::new(w - 0.12, ph - 0.008, 0.022), 0.006);
            put(&mut face, &p, Vec3::new(0.0, y, d - 0.035));
        }
        // Frame: rails and stiles flush with the outside.
        for y in [-h.y + 0.05, h.y - 0.05] {
            let p = plank(rng, Vec3::new(w, 0.1, 0.026), 0.008);
            put(&mut face, &p, Vec3::new(0.0, y, d - 0.013));
        }
        for x in [-w * 0.5 + 0.05, w * 0.5 - 0.05] {
            let p = post(rng, Vec3::new(0.1, size.y - 0.2, 0.026), 0.008);
            put(&mut face, &p, Vec3::new(x, 0.0, d - 0.013));
        }
        let (bw, bh) = (w - 0.2, size.y - 0.2);
        let len = (bw * bw + bh * bh).sqrt();
        let p = plank(rng, Vec3::new(len, 0.1, 0.022), 0.006);
        append(
            &mut face,
            &p,
            Mat4::from_rotation_translation(
                Quat::from_rotation_z(bh.atan2(bw) * if k % 2 == 0 { 1.0 } else { -1.0 }),
                Vec3::new(0.0, 0.0, d - 0.016),
            ),
        );
        // Nails at the frame crossings, corner plates.
        for x in [-w * 0.5 + 0.05, w * 0.5 - 0.05] {
            for y in [-h.y + 0.05, h.y - 0.05, 0.0] {
                let s = stud_mesh(0.009);
                append(
                    &mut fi,
                    &s,
                    Mat4::from_rotation_translation(
                        Quat::from_rotation_x(FRAC_PI_2),
                        Vec3::new(x + 0.02, y, d + 0.0),
                    ),
                );
            }
            for y in [-h.y + 0.075, h.y - 0.075] {
                let plate = rounded_box(Vec3::new(0.075, 0.075, 0.003), BoxOpts::round(0.004));
                let sx = if x < 0.0 {
                    -w * 0.5 + 0.075
                } else {
                    w * 0.5 - 0.075
                };
                put(&mut fi, &plate, Vec3::new(sx, y, d + 0.003));
                let s = stud_mesh(0.011);
                append(
                    &mut fi,
                    &s,
                    Mat4::from_rotation_translation(
                        Quat::from_rotation_x(FRAC_PI_2),
                        Vec3::new(sx, y, d + 0.006),
                    ),
                );
            }
        }
        append(&mut wood, &face, Mat4::from_quat(rot));
        append(&mut iron, &fi, Mat4::from_quat(rot));
    }
    // Lid: planks along X with two battens, and a bottom.
    let lid_n = 6;
    let pw = (size.z - 0.02) / lid_n as f32;
    for j in 0..lid_n {
        let z = -h.z + 0.01 + pw * (j as f32 + 0.5);
        let p = plank(rng, Vec3::new(size.x - 0.01, 0.026, pw - 0.008), 0.006);
        put(&mut wood, &p, Vec3::new(0.0, h.y - 0.013, z));
    }
    for x in [-h.x + 0.25, h.x - 0.25] {
        let p = plank(rng, Vec3::new(size.z - 0.06, 0.024, 0.1), 0.006);
        append(
            &mut wood,
            &p,
            Mat4::from_rotation_translation(
                Quat::from_rotation_y(FRAC_PI_2),
                Vec3::new(x, h.y + 0.012, 0.0),
            ),
        );
        for z in [-h.z + 0.1, 0.0, h.z - 0.1] {
            put(&mut iron, &stud_mesh(0.01), Vec3::new(x, h.y + 0.024, z));
        }
    }
    let core = rounded_box(h - Vec3::splat(0.04), BoxOpts::round(0.01));
    let mut core = core;
    tint(&mut core, Vec4::new(0.3, 0.27, 0.24, 1.0));
    wood.merge(&core);
    (wood, iron)
}

/// Two crates stacked for the 1.6 × 2.2 × 1.6 m crate colliders (origin at the collider's
/// center), the top one turned a little. Returns (wood, iron).
pub fn crate_stack_meshes(seed: u64) -> (Mesh, Mesh) {
    let mut rng = Rng::new(seed);
    let (w0, i0) = crate_box(&mut rng, Vec3::new(1.6, 1.1, 1.6));
    let (w1, i1) = crate_box(&mut rng, Vec3::new(1.54, 1.1, 1.54));
    let twist = Quat::from_rotation_y(rng.signed() * 0.05);
    let mut wood = transformed(&w0, Mat4::from_translation(Vec3::new(0.0, -0.55, 0.0)));
    let mut iron = transformed(&i0, Mat4::from_translation(Vec3::new(0.0, -0.55, 0.0)));
    append(
        &mut wood,
        &w1,
        Mat4::from_rotation_translation(twist, Vec3::new(0.0, 0.55, 0.0)),
    );
    append(
        &mut iron,
        &i1,
        Mat4::from_rotation_translation(twist, Vec3::new(0.0, 0.55, 0.0)),
    );
    grime(&mut wood, -1.1, 0.4, 0.35);
    (wood, iron)
}

/// A 200-liter steel drum (radius 0.32, 1 m, origin at its center) with rolled chimes, two
/// rolling hoops, recessed lids with bungs and a few dents, mapped for [`barrel_set`].
pub fn barrel_mesh(seed: u64) -> Mesh {
    let mut rng = Rng::new(seed);
    let dents: Vec<(f32, f32, f32)> = (0..3)
        .map(|_| {
            (
                rng.range(0.0, TAU),
                rng.range(-0.4, 0.4),
                rng.range(0.006, 0.014),
            )
        })
        .collect();
    let dent = move |a: f32, y: f32, r: f32| {
        let mut r = r;
        for &(da, dy, depth) in &dents {
            let mut d = (a - da).rem_euclid(TAU);
            if d > PI {
                d -= TAU;
            }
            let q = Vec2::new(d * 0.3 / 0.09, (y - dy) / 0.09);
            r -= depth * (-q.length_squared()).exp();
        }
        r
    };
    let mut body: Vec<Vec2> = Vec::new();
    // Bottom chime (rolled seam), up the body with two hoops, top chime.
    body.extend(arc_points(
        Vec2::new(0.308, -0.488),
        0.012,
        PI,
        2.0 * PI + FRAC_PI_2,
        8,
    ));
    body.extend([Vec2::new(0.302, -0.465), Vec2::new(0.3, -0.45)]);
    for hy in [-0.17f32, 0.17] {
        body.push(Vec2::new(0.3, hy - 0.04));
        body.extend(
            arc_points(Vec2::new(0.3, hy), 0.018, -FRAC_PI_2, FRAC_PI_2, 6)
                .into_iter()
                .map(|p| Vec2::new(0.3 + (p.x - 0.3) * 0.9, p.y)),
        );
        body.push(Vec2::new(0.3, hy + 0.04));
    }
    body.extend([Vec2::new(0.3, 0.45), Vec2::new(0.302, 0.465)]);
    body.extend(arc_points(
        Vec2::new(0.308, 0.488),
        0.012,
        -FRAC_PI_2,
        PI,
        8,
    ));
    body.push(Vec2::new(0.292, 0.47));
    let mut m = lathe(&[&body], 56, 1.0 / TAU, &dent);
    // Side mapping: u around (0..1), v from the top rim down over 0..0.68.
    for v in &mut m.vertices {
        let y = v.position[1];
        v.uv[1] = (0.5 - y) * 0.68;
    }
    // Lids, slightly recessed, mapped into the square at u 0..0.32, v 0..0.68.
    for (y, up) in [(0.47f32, true), (-0.47, false)] {
        let pts = if up {
            vec![Vec2::new(0.292, y), Vec2::new(0.0, y)]
        } else {
            vec![Vec2::new(0.0, y), Vec2::new(0.292, y)]
        };
        let mut lid = lathe(&[&pts], 56, 1.0, &|_, _, r| r);
        for v in &mut lid.vertices {
            let (x, z) = (v.position[0], v.position[2]);
            v.uv = [0.16 + x * 0.5, 0.84 + z * 0.5];
        }
        m.merge(&lid);
    }
    // Bungs on the top lid.
    for (at, r) in [
        (Vec2::new(0.19, 0.0), 0.035f32),
        (Vec2::new(-0.2, 0.06), 0.022),
    ] {
        let mut b = lathe(
            &[&[
                Vec2::new(r, 0.47),
                Vec2::new(r, 0.482),
                Vec2::new(r * 0.7, 0.486),
                Vec2::new(0.0, 0.486),
            ]],
            12,
            1.0,
            &|_, _, r| r,
        );
        for v in &mut b.vertices {
            let (x, z) = (v.position[0] + at.x, v.position[2] + at.y);
            v.uv = [0.16 + x * 0.5, 0.84 + z * 0.5];
        }
        put(&mut m, &b, Vec3::new(at.x, 0.0, at.y));
    }
    m
}

/// A beam along Z (grain along its length) of `size` with soft edges.
fn beam_z(rng: &mut Rng, size: Vec3, round: f32) -> Mesh {
    let p = plank(rng, Vec3::new(size.z, size.y, size.x), round);
    transformed(&p, Mat4::from_rotation_y(FRAC_PI_2))
}

/// Swap texture u and v (grain along a lathe's profile instead of around it).
fn swap_uv(m: &mut Mesh) {
    for v in &mut m.vertices {
        v.uv = [v.uv[1], v.uv[0]];
    }
}

/// A flat slab with the outline `outline` (x, z; any winding) from `y0` to `y1`: top face and
/// side walls, texture coordinates in meters from above.
fn prism(outline: &[Vec2], y0: f32, y1: f32) -> Mesh {
    let n = outline.len();
    let area: f32 = (0..n)
        .map(|i| {
            let (a, b) = (outline[i], outline[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum();
    // Walk the outline so it turns clockwise seen from above (+Y), i.e. counter-clockwise in
    // (x, z), which keeps the faces wound outward below.
    let pts: Vec<Vec2> = if area > 0.0 {
        outline.to_vec()
    } else {
        outline.iter().rev().copied().collect()
    };
    let mut m = Mesh::default();
    let c = pts.iter().copied().sum::<Vec2>() / n as f32;
    let base = m.vertices.len() as u32;
    m.vertices
        .push(Vertex::new(Vec3::new(c.x, y1, c.y), Vec3::Y, c));
    for p in &pts {
        m.vertices
            .push(Vertex::new(Vec3::new(p.x, y1, p.y), Vec3::Y, *p));
    }
    for i in 0..n as u32 {
        let (a, b) = (base + 1 + i, base + 1 + (i + 1) % n as u32);
        m.indices.extend_from_slice(&[base, b, a]);
    }
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let d = b - a;
        let normal = Vec3::new(d.y, 0.0, -d.x).normalize_or(Vec3::X);
        let len = d.length();
        let base = m.vertices.len() as u32;
        for (p, u) in [(a, 0.0), (b, len)] {
            for y in [y0, y1] {
                m.vertices.push(Vertex::new(
                    Vec3::new(p.x, y, p.y),
                    normal,
                    Vec2::new(u, -y),
                ));
            }
        }
        m.indices
            .extend_from_slice(&[base, base + 1, base + 3, base, base + 3, base + 2]);
    }
    m
}

// ============================================================================== high ground

/// The raised platform for its collider (origin at the collider's center): a plinth course
/// 3 cm proud reaching into the ground, two courses of ashlar whose corners alternate between
/// the faces, and a paved top with a rounded nosing that overhangs 3 cm.
pub fn platform_mesh(half: Vec3, seed: u64) -> Mesh {
    let mut ms = Mason::new(seed, Tone::Sand);
    let (hx, hy, hz) = (half.x, half.y, half.z);
    let d = 0.45;
    let courses = [
        (-hy - 0.25, -hy + 0.18, 0.03),
        (-hy + 0.18, -0.06, 0.0),
        (-0.06, hy - 0.3, 0.0),
    ];
    for (i, &(y0, y1, out)) in courses.iter().enumerate() {
        let (ex, ez) = (hx + out, hz + out);
        // ±X walls take the corners on even courses, ±Z walls on odd ones.
        let x_corners = i % 2 == 0;
        for s in [-1.0f32, 1.0] {
            let (za, zb) = if x_corners {
                (-ez, ez)
            } else {
                (-ez + d, ez - d)
            };
            let (xa, xb) = if s > 0.0 {
                (ex - d, ex)
            } else {
                (-ex, -ex + d)
            };
            for (a, b) in run(&mut ms.rng, za, zb, 0.8, 1.4) {
                ms.block(
                    Vec3::new(xa, y0, a),
                    Vec3::new(xb, y1, b),
                    sb(0.03).skip(NEG_Y),
                );
            }
            let (xa, xb) = if x_corners {
                (-ex + d, ex - d)
            } else {
                (-ex, ex)
            };
            let (za, zb) = if s > 0.0 {
                (ez - d, ez)
            } else {
                (-ez, -ez + d)
            };
            ms.course(
                (xa, xb),
                (y0, y1),
                (za, zb),
                (0.8, 1.4),
                sb(0.03).skip(NEG_Y),
            );
        }
    }
    let (ex, ez) = (hx + 0.03, hz + 0.03);
    ms.lid = NEG_Y;
    for (za, zb) in run(&mut ms.rng, -ez, ez, 0.8, 1.25) {
        ms.course(
            (-ex, ex),
            (hy - 0.3, hy),
            (za, zb),
            (0.8, 1.4),
            pb(0.045).skip(NEG_Y),
        );
    }
    let mut m = ms.mesh;
    grime(&mut m, -hy, 0.5, 0.35);
    m
}

/// A stone step for a step collider (origin at its center; it is climbed toward −X): coursed
/// blocks under a tread slab whose rounded nosing overhangs the +X riser by 3 cm.
pub fn step_mesh(half: Vec3, seed: u64) -> Mesh {
    let mut ms = Mason::new(seed, Tone::Sand);
    let (hx, hy, hz) = (half.x, half.y, half.z);
    let tread = 0.15;
    let (y0, y1) = (-hy - 0.25, hy - tread);
    let n = ((y1 - y0) / 0.45).round().max(1.0) as usize;
    for k in 0..n {
        let a = y0 + (y1 - y0) * k as f32 / n as f32;
        let b = y0 + (y1 - y0) * (k + 1) as f32 / n as f32;
        for (za, zb) in run(&mut ms.rng, -hz, hz, 0.7, 1.3) {
            ms.block(
                Vec3::new(-hx, a, za),
                Vec3::new(hx, b, zb),
                pb(0.03).skip(NEG_Y),
            );
        }
    }
    ms.lid = NEG_Y;
    for (za, zb) in run(&mut ms.rng, -hz - 0.02, hz + 0.02, 0.9, 1.4) {
        ms.block(
            Vec3::new(-hx, y1, za),
            Vec3::new(hx + 0.03, hy, zb),
            pb(0.045).skip(NEG_Y),
        );
    }
    let mut m = ms.mesh;
    grime(&mut m, -hy, 0.4, 0.3);
    m
}

/// The timber ramp for its collider (origin at the collider's center, `half` its half extents,
/// deck on top in the collider's tilted frame; `tilt` is its pitch about X, high end toward
/// −Z, and `ground` the floor's height below the origin, measured upright): three stringers, a
/// deck of planks across the slope with low cleats for footing and two nails per crossing, an
/// iron nosing at the high end, and a trestle of two legs with knee braces under it.
/// Returns (wood, iron).
pub fn ramp_meshes(half: Vec3, tilt: f32, ground: f32, seed: u64) -> (Mesh, Mesh) {
    let mut rng = Rng::new(seed);
    let mut wood = Mesh::default();
    let mut iron = Mesh::default();
    let (hx, hy, hz) = (half.x, half.y, half.z);
    let deck = 0.075;
    let xs = [-hx + 0.16, 0.0, hx - 0.16];
    for &x in &xs {
        let b = beam_z(
            &mut rng,
            Vec3::new(0.2, 2.0 * hy - deck, 2.0 * hz - 0.03),
            0.02,
        );
        put(&mut wood, &b, Vec3::new(x, -deck * 0.5, 0.0));
    }
    let pitch = 0.25;
    let n = (2.0 * hz / pitch).round() as usize;
    let pitch = 2.0 * hz / n as f32;
    for i in 0..n {
        let z = -hz + pitch * (i as f32 + 0.5);
        let len = 2.0 * hx - 0.03 + rng.signed() * 0.02;
        let p = plank(&mut rng, Vec3::new(len, deck, pitch - 0.012), 0.012);
        let rot = Quat::from_euler(
            EulerRot::YXZ,
            rng.signed() * 0.006,
            0.0,
            rng.signed() * 0.004,
        );
        let at = Vec3::new(rng.signed() * 0.012, hy - deck * 0.5 - rng.f() * 0.004, z);
        append(&mut wood, &p, Mat4::from_rotation_translation(rot, at));
        for &x in &xs {
            let dz = if (i + x as usize).is_multiple_of(2) {
                -0.06
            } else {
                0.06
            };
            let at = Vec3::new(x + rng.signed() * 0.02, hy - 0.001, z + dz);
            put(&mut iron, &stud_mesh(0.011), at);
        }
        // A cleat every third plank.
        if i % 3 == 1 && i + 2 < n {
            let c = plank(&mut rng, Vec3::new(2.0 * hx - 0.5, 0.028, 0.06), 0.008);
            put(&mut wood, &c, Vec3::new(0.0, hy + 0.014, z + pitch * 0.5));
            for &x in &xs {
                put(
                    &mut iron,
                    &stud_mesh(0.01),
                    Vec3::new(x, hy + 0.027, z + pitch * 0.5),
                );
            }
        }
    }
    // Iron nosing over the high end.
    let top = rounded_box(Vec3::new(hx, 0.004, 0.05), BoxOpts::round(0.003));
    put(&mut iron, &top, Vec3::new(0.0, hy + 0.003, -hz + 0.05));
    let lip = rounded_box(Vec3::new(hx, 0.06, 0.004), BoxOpts::round(0.003));
    put(&mut iron, &lip, Vec3::new(0.0, hy - 0.055, -hz - 0.003));
    // Trestle: a header under the stringers near the high end, and two upright legs, built
    // upright and tilted back into the ramp's frame.
    let zl = -hz + 0.45;
    let header = plank(&mut rng, Vec3::new(2.0 * hx + 0.12, 0.2, 0.2), 0.02);
    put(&mut wood, &header, Vec3::new(0.0, -hy - 0.1, zl));
    let to_local = Mat4::from_rotation_x(-tilt);
    let up = Mat4::from_rotation_x(tilt);
    for s in [-1.0f32, 1.0] {
        let x = s * (hx - 0.16);
        let q = up.transform_point3(Vec3::new(x, -hy - 0.2, zl));
        let (y_top, y_bot) = (q.y + 0.05, ground - 0.3);
        let leg = post(&mut rng, Vec3::new(0.2, y_top - y_bot, 0.2), 0.02);
        let m = to_local * Mat4::from_translation(Vec3::new(x, (y_top + y_bot) * 0.5, q.z));
        append(&mut wood, &leg, m);
        // A knee brace from the leg up to the header, inward.
        let a = Vec3::new(x, q.y - 0.75, q.z);
        let b = Vec3::new(x - s * 0.75, q.y - 0.02, q.z);
        let len = a.distance(b);
        let brace = plank(&mut rng, Vec3::new(len + 0.1, 0.14, 0.12), 0.015);
        let ang = (b.y - a.y).atan2(b.x - a.x);
        let m =
            to_local * Mat4::from_rotation_translation(Quat::from_rotation_z(ang), (a + b) * 0.5);
        append(&mut wood, &brace, m);
        for at in [a, b] {
            let bolt = transformed(&stud_mesh(0.02), Mat4::from_rotation_x(-FRAC_PI_2));
            append(
                &mut iron,
                &bolt,
                to_local * Mat4::from_translation(at + Vec3::new(0.0, 0.0, -0.06)),
            );
        }
    }
    grime(&mut wood, -hy - 0.5, 0.6, 0.2);
    (wood, iron)
}

/// The wall-run wall for its collider (origin at its center): a footing course 3 cm proud and
/// 35 cm into the ground, ten courses of through-stones with a few millimeters of play, and
/// a coping that overhangs 3.5 cm.
pub fn wall_run_mesh(half: Vec3, seed: u64) -> Mesh {
    let mut ms = Mason::new(seed, Tone::Sand);
    let (hx, hy, hz) = (half.x, half.y, half.z);
    let foot = (-hy - 0.35, -hy + 0.15);
    ms.course(
        (-hz - 0.03, hz + 0.03),
        foot,
        (-hx - 0.03, hx + 0.03),
        (0.9, 1.6),
        pb(0.035).skip(NEG_Y),
    );
    let (y0, y1) = (foot.1, hy - 0.25);
    let n = 10;
    for k in 0..n {
        let a = y0 + (y1 - y0) * k as f32 / n as f32;
        let b = y0 + (y1 - y0) * (k + 1) as f32 / n as f32;
        for (za, zb) in run(&mut ms.rng, -hz, hz, 0.75, 1.5) {
            let j = ms.rng.signed() * 0.006;
            ms.course(
                (za, zb),
                (a, b),
                (-hx + j, hx + j),
                (10.0, 10.0),
                sb(0.03).skip(NEG_Y),
            );
        }
    }
    ms.lid = NEG_Y;
    ms.course(
        (-hz - 0.035, hz + 0.035),
        (y1, hy),
        (-hx - 0.035, hx + 0.035),
        (1.0, 1.6),
        pb(0.05).skip(NEG_Y),
    );
    // Built along X; turn it so its length runs along Z like the collider.
    let mut m = transformed(&ms.mesh, Mat4::from_rotation_y(FRAC_PI_2));
    grime(&mut m, -hy, 0.7, 0.4);
    m
}

// ============================================================================== braziers

/// A brazier (origin on the floor): a chipped stone slab and a molded drum, a forged tripod
/// with scrolled feet and a tie ring, a hammered bowl with a rolled rim, and a heap of coals
/// (mapped for [`coal_set`]). Returns (stone, iron, coal).
pub fn brazier_meshes(seed: u64) -> (Mesh, Mesh, Mesh) {
    let mut rng = Rng::new(seed);
    let s32 = seed as u32;
    let wear = move |a: f32, y: f32, r: f32| {
        let p = Vec3::new(a.cos() * r, y, a.sin() * r);
        r - 0.008 * smooth(0.1, 0.8, vn3(p * 9.0, s32)) - 0.002 * vn3(p * 30.0, s32 ^ 3)
    };
    let mut slab = chipped_block(
        Vec3::new(0.5, 0.11, 0.5),
        Chip {
            voxel: 0.04,
            depth: 0.04,
            breaks: 2,
            ..Chip::new(seed)
        },
    );
    tint(&mut slab, tone(&mut rng, Tone::Sand));
    let mut stone = transformed(&slab, Mat4::from_translation(Vec3::new(0.0, 0.07, 0.0)));
    let base: Vec<Vec2> = {
        let mut v = vec![Vec2::new(0.4, 0.17)];
        v.extend(arc_points(
            Vec2::new(0.4, 0.22),
            0.05,
            -FRAC_PI_2,
            FRAC_PI_2,
            8,
        ));
        v.push(Vec2::new(0.37, 0.27));
        v
    };
    let shaft = [
        Vec2::new(0.37, 0.27),
        Vec2::new(0.36, 0.45),
        Vec2::new(0.355, 0.62),
    ];
    let mut cap = vec![Vec2::new(0.355, 0.62), Vec2::new(0.385, 0.635)];
    cap.extend(arc_points(
        Vec2::new(0.39, 0.665),
        0.03,
        -FRAC_PI_2 * 0.8,
        FRAC_PI_2,
        6,
    ));
    cap.extend([
        Vec2::new(0.42, 0.695),
        Vec2::new(0.42, 0.715),
        Vec2::new(0.41, 0.725),
    ]);
    cap.push(Vec2::new(0.0, 0.725));
    let mut drum = lathe(&[&base, &shaft, &cap], 48, 0.4, &wear);
    tint(&mut drum, tone(&mut rng, Tone::Lime));
    stone.merge(&drum);
    grime(&mut stone, 0.0, 0.3, 0.3);
    // Coal soot rising on the drum.
    paint(&mut stone, |p, _| {
        let k = 1.0 - 0.3 * smooth(0.4, 0.73, p.y);
        Vec4::new(k, k, k, 1.0)
    });

    let mut iron = Mesh::default();
    let turn = rng.range(0.0, TAU);
    for k in 0..3 {
        let th = turn + k as f32 * TAU / 3.0;
        let mut path: Vec<Vec2> = vec![
            Vec2::new(0.462, 1.09),
            Vec2::new(0.44, 1.04),
            Vec2::new(0.38, 0.97),
            Vec2::new(0.33, 0.91),
            Vec2::new(0.3, 0.85),
            Vec2::new(0.29, 0.81),
        ];
        let c = Vec2::new(0.34, 0.78);
        for j in 1..=16 {
            let t = j as f32 / 16.0;
            let a = PI + t * 1.7 * PI;
            let r = 0.05 - 0.032 * t;
            path.push(c + Vec2::new(a.cos(), a.sin()) * r);
        }
        let pts: Vec<Vec3> = path
            .iter()
            .map(|p| Vec3::new(th.cos() * p.x, p.y, -th.sin() * p.x))
            .collect();
        iron.merge(&tube(&pts, &|t| 0.028 - 0.012 * t, 8, true));
    }
    let ring: Vec<Vec3> = (0..=48)
        .map(|i| {
            let a = i as f32 / 48.0 * TAU;
            Vec3::new(a.cos() * 0.322, 0.9, -a.sin() * 0.322)
        })
        .collect();
    iron.merge(&tube(&ring, &|_| 0.011, 6, false));
    // Central stem.
    let stem = lathe(
        &[&[
            Vec2::new(0.07, 0.72),
            Vec2::new(0.07, 0.75),
            Vec2::new(0.045, 0.77),
            Vec2::new(0.04, 0.95),
            Vec2::new(0.0, 0.95),
        ]],
        12,
        0.05,
        &|_, _, r| r,
    );
    iron.merge(&stem);
    // Bowl: hammered outside, rolled rim, inside back down.
    let (cy, rx, ry) = (1.16f32, 0.5f32, 0.22f32);
    let mut outer: Vec<Vec2> = (0..=14)
        .map(|k| {
            let t = -FRAC_PI_2 + FRAC_PI_2 * k as f32 / 14.0;
            Vec2::new(rx * t.cos(), cy + ry * t.sin())
        })
        .collect();
    outer[0].x = 0.0;
    let bead = arc_points(Vec2::new(0.51, cy + 0.01), 0.018, -PI * 0.75, PI, 10);
    let inner: Vec<Vec2> = (0..=12)
        .map(|k| {
            let t = -FRAC_PI_2 * k as f32 / 12.0;
            Vec2::new((rx - 0.012) * t.cos(), cy + 0.01 + (ry - 0.012) * t.sin())
        })
        .collect();
    let hammer = move |a: f32, y: f32, r: f32| {
        let p = Vec3::new(a.cos() * r, y, a.sin() * r);
        r - 0.003 * smooth(-0.2, 0.6, vn3(p * 22.0, s32 ^ 0x44))
    };
    let mut bowl = lathe(&[&outer, &bead, &inner], 56, 0.5, &hammer);
    paint(&mut bowl, |p, _| {
        // Heat-blued and sooty toward the rim.
        let k = 1.0 - 0.35 * smooth(1.0, 1.18, p.y);
        Vec4::new(k * 0.95, k * 0.97, k, 1.0)
    });
    iron.merge(&bowl);

    // Coals: a lumpy heap inside the bowl.
    let rs = steps(0.0, 1.0, 16);
    let angles = steps(0.0, TAU, 48);
    let heap = move |t: f32, a: f32| {
        let rho = 0.475 * t;
        let p = Vec3::new(a.cos() * rho, 0.0, -a.sin() * rho);
        let lumps =
            0.022 * vn3(p * 18.0 + Vec3::splat(3.3), s32 ^ 0xc0) + 0.01 * vn3(p * 40.0, s32 ^ 0xc1);
        let y = 1.11 + 0.075 * (1.0 - t * t) + lumps * (1.0 - t * 0.6);
        Vec3::new(p.x, y, p.z)
    };
    let coal = surface(&rs, &angles, &heap, &|_, _, p| Vec2::new(p.x, p.z));
    (stone, iron, coal)
}

// ============================================================================== banners

/// A banner pole (origin at its foot on the walkway, the cloth hanging on the −Z side): an
/// iron socket and pole, and in brass a spear finial over a ball, the crossbar with knobs, its
/// bracket and the rings the cloth hangs from. Returns (iron, brass).
pub fn banner_pole_meshes() -> (Mesh, Mesh) {
    let socket = [
        Vec2::new(0.12, -0.05),
        Vec2::new(0.12, 0.04),
        Vec2::new(0.1, 0.06),
        Vec2::new(0.085, 0.3),
        Vec2::new(0.07, 0.33),
        Vec2::new(0.05, 0.34),
    ];
    let pole = [
        Vec2::new(0.05, 0.34),
        Vec2::new(0.045, 7.0),
        Vec2::new(0.0, 7.0),
    ];
    let mut iron = lathe(&[&socket, &pole], 16, 0.05, &|_, _, r| r);
    swap_uv(&mut iron);
    // Bracket from the pole to the crossbar.
    let bracket = tube(
        &[
            Vec3::new(0.0, 6.62, 0.0),
            Vec3::new(0.0, 6.55, -0.06),
            Vec3::new(0.0, 6.45, -0.12),
        ],
        &|_| 0.018,
        8,
        true,
    );
    iron.merge(&bracket);

    let mut brass = Mesh::default();
    let mut fin: Vec<Vec2> = vec![Vec2::new(0.055, 6.98), Vec2::new(0.06, 7.0)];
    fin.extend(arc_points(
        Vec2::new(0.0, 7.07),
        0.065,
        -FRAC_PI_2 * 0.3,
        FRAC_PI_2 * 0.75,
        8,
    ));
    fin.push(Vec2::new(0.02, 7.16));
    let blade = [
        Vec2::new(0.02, 7.16),
        Vec2::new(0.055, 7.24),
        Vec2::new(0.06, 7.3),
        Vec2::new(0.04, 7.4),
        Vec2::new(0.0, 7.5),
    ];
    let flat = |a: f32, y: f32, r: f32| {
        if y > 7.165 {
            r * (0.16 + 0.84 * a.cos().abs().powf(1.5))
        } else {
            r
        }
    };
    brass.merge(&lathe(&[&fin, &blade], 24, 0.05, &flat));
    let (bar_y, bar_z) = (6.45, -0.12);
    let bar: Vec<Vec3> = steps(-0.86, 0.86, 8)
        .into_iter()
        .map(|x| Vec3::new(x, bar_y, bar_z))
        .collect();
    brass.merge(&tube(&bar, &|_| 0.026, 12, false));
    for s in [-1.0f32, 1.0] {
        let mut k: Vec<Vec2> = vec![Vec2::new(0.035, -0.02)];
        k.extend(arc_points(
            Vec2::new(0.0, 0.04),
            0.05,
            -FRAC_PI_2 * 0.5,
            FRAC_PI_2 * 0.6,
            8,
        ));
        k.extend([Vec2::new(0.012, 0.11), Vec2::new(0.0, 0.13)]);
        let knob = lathe(&[&k], 16, 0.05, &|_, _, r| r);
        let m = Mat4::from_translation(Vec3::new(s * 0.86, bar_y, bar_z))
            * Mat4::from_rotation_z(-s * FRAC_PI_2);
        append(&mut brass, &knob, m);
    }
    for i in 0..6 {
        let x = -0.68 + 0.272 * i as f32;
        let ring: Vec<Vec3> = (0..=16)
            .map(|k| {
                let a = k as f32 / 16.0 * TAU;
                Vec3::new(x, bar_y - 0.02 + a.cos() * 0.045, bar_z + a.sin() * 0.045)
            })
            .collect();
        brass.merge(&tube(&ring, &|_| 0.006, 5, false));
    }
    (iron, brass)
}

/// The banner cloth, hung from y = 6.4 at z = −0.12 in the pole's frame and built upside down
/// (turned by π about Z; give its entity that rotation) so wind sway, which grows from the
/// mesh's lowest point, moves the free bottom edge and keeps the top still. Heavy folds that
/// deepen downward, a swallowtail and slightly curling edges; mapped for [`banner_set`] seen
/// from the arena (−Z).
pub fn banner_cloth_mesh(seed: u64) -> Mesh {
    let (w, h, top, z0) = (1.5f32, 3.4f32, 6.4f32, -0.12f32);
    let s32 = seed as u32;
    let notch = 0.12;
    let drop = move |u: f32| h * (1.0 - notch * (1.0 - (2.0 * u - 1.0).abs()));
    let phase = Rng::new(seed).range(0.0, TAU);
    let f = move |u: f32, v: f32| {
        let y = top - v * drop(u);
        let x = (u - 0.5) * w * (1.0 - 0.025 * v);
        let fold = (TAU * (u * 2.6) + phase).sin() * (0.012 + 0.05 * v * v)
            + (TAU * (u * 5.3) + phase * 1.7).sin() * 0.008 * v;
        let curl = 0.03 * (2.0 * u - 1.0).abs().powi(3) * (0.4 + v);
        let ripple = 0.01 * vn3(Vec3::new(u * 6.0, v * 9.0, 0.0), s32) * v;
        // The tails twist a little.
        let tail = 0.04 * smooth(0.75, 1.0, v) * (u - 0.5).signum() * (u - 0.5).abs() * 2.0;
        Vec3::new(x, y, z0 + fold + curl + ripple + tail)
    };
    let us = steps(0.0, 1.0, 16);
    let vs = steps(0.0, 1.0, 40);
    let m = surface(&us, &vs, &f, &|u, v, _| Vec2::new(1.0 - u, v * drop(u) / h));
    transformed(&m, Mat4::from_rotation_z(PI))
}

// ============================================================================== duel ring

/// The duel ring (origin at its center on the floor): 24 curved limestone curb stones from
/// 4.82 to 5.18 m standing 3.5 cm proud of the sand, a brass inlay ring inside them, and a
/// center medallion with a brass compass star. Returns (stone, brass).
pub fn ring_meshes(seed: u64) -> (Mesh, Mesh) {
    let mut ms = Mason::new(seed, Tone::Lime);
    let (r, n) = (5.0f32, 24);
    let gap = 0.004;
    for k in 0..n {
        let am = (k as f32 + 0.5) / n as f32 * TAU;
        let half = Vec3::new(PI * r / n as f32 - gap, 0.08, 0.18);
        let mut b = rounded_box(half, BoxOpts::round(0.025).bevel(2).step(0.2).skip(NEG_Y));
        let lift = ms.rng.range(-0.004, 0.004);
        deform(&mut b, |p| {
            let a = am - p.x / r;
            let rho = r + p.z;
            Vec3::new(rho * a.cos(), 0.035 - 0.08 + p.y + lift, rho * a.sin())
        });
        ms.add(b, Mat4::IDENTITY);
    }
    // Medallion: a round slab with a chamfered edge.
    let disc = lathe(
        &[&[
            Vec2::new(0.72, -0.08),
            Vec2::new(0.72, 0.012),
            Vec2::new(0.705, 0.028),
            Vec2::new(0.69, 0.032),
            Vec2::new(0.0, 0.032),
        ]],
        64,
        0.7,
        &|_, _, r| r,
    );
    let mut disc = disc;
    for v in &mut disc.vertices {
        v.uv = [v.position[0], v.position[2]];
    }
    ms.add(disc, Mat4::IDENTITY);
    let mut stone = ms.mesh;
    grime(&mut stone, 0.0, 0.03, 0.1);

    let mut brass = Mesh::default();
    let band = |r0: f32, r1: f32, segments: u32| {
        let mut b = lathe(
            &[&[
                Vec2::new(r1, -0.02),
                Vec2::new(r1, 0.03),
                Vec2::new(r0, 0.03),
                Vec2::new(r0, -0.02),
            ]],
            segments,
            1.0,
            &|_, _, r| r,
        );
        for v in &mut b.vertices {
            v.uv = [v.position[0], v.position[2]];
        }
        b
    };
    brass.merge(&band(4.7, 4.76, 192));
    brass.merge(&band(0.75, 0.79, 48));
    // Compass star: eight points, the four cardinal ones longer.
    let mut star: Vec<Vec2> = Vec::new();
    for k in 0..16 {
        let a = k as f32 / 16.0 * TAU;
        let r = if k % 2 == 1 {
            0.12
        } else if k % 4 == 0 {
            0.6
        } else {
            0.4
        };
        star.push(Vec2::new(a.cos(), a.sin()) * r);
    }
    brass.merge(&prism(&star, 0.0, 0.036));
    (stone, brass)
}

// ============================================================================== floor

// ============================================================================== sparring

/// A sparring mat (origin on the floor at its center, 3.6 × 2.6 m): a woven rush mat that
/// follows the sand a little, bound with a leather border. Returns (mat, leather).
pub fn mat_meshes(seed: u64) -> (Mesh, Mesh) {
    let s32 = seed as u32;
    let (hx, hz) = (1.8f32, 1.3f32);
    let sag = move |p: Vec3| p + Vec3::Y * 0.004 * vn3(p * 1.7, s32);
    let mut mat = rounded_box(
        Vec3::new(hx - 0.06, 0.017, hz - 0.06),
        BoxOpts::round(0.008).step(0.15).skip(NEG_Y),
    );
    deform(&mut mat, |p| sag(p + Vec3::Y * 0.02));
    let mut leather = Mesh::default();
    for (half, at) in [
        (Vec3::new(hx, 0.02, 0.07), Vec3::new(0.0, 0.024, hz - 0.07)),
        (Vec3::new(hx, 0.02, 0.07), Vec3::new(0.0, 0.024, -hz + 0.07)),
        (
            Vec3::new(0.07, 0.02, hz - 0.14),
            Vec3::new(hx - 0.07, 0.024, 0.0),
        ),
        (
            Vec3::new(0.07, 0.02, hz - 0.14),
            Vec3::new(-hx + 0.07, 0.024, 0.0),
        ),
    ] {
        let mut b = rounded_box(half, BoxOpts::round(0.012).bevel(2).step(0.2).skip(NEG_Y));
        deform(&mut b, |p| sag(p + at));
        leather.merge(&b);
    }
    (mat, leather)
}

/// A sparring post's static body (origin at its foot): a round stone base, a turned post with
/// a leather grip wrap, iron bands and the pivot collar the arm swings on, and a carved head.
/// Returns (stone, wood, iron, leather).
pub fn dummy_meshes(seed: u64) -> (Mesh, Mesh, Mesh, Mesh) {
    let s32 = seed as u32;
    let wear = move |a: f32, y: f32, r: f32| {
        let p = Vec3::new(a.cos() * r, y, a.sin() * r);
        r - 0.01 * smooth(0.1, 0.8, vn3(p * 8.0, s32)) - 0.002 * vn3(p * 30.0, s32 ^ 7)
    };
    let mut base = vec![Vec2::new(0.46, -0.05), Vec2::new(0.46, 0.07)];
    base.extend(arc_points(Vec2::new(0.42, 0.07), 0.04, 0.0, FRAC_PI_2, 5));
    base.extend([
        Vec2::new(0.3, 0.135),
        Vec2::new(0.2, 0.17),
        Vec2::new(0.0, 0.17),
    ]);
    let mut stone = lathe(&[&base], 48, 0.45, &wear);
    for v in &mut stone.vertices {
        if v.position[1] > 0.1 {
            v.uv = [v.position[0], v.position[2]];
        }
    }
    let mut post = vec![
        Vec2::new(0.15, 0.15),
        Vec2::new(0.145, 0.8),
        Vec2::new(0.135, 1.62),
    ];
    post.extend(arc_points(Vec2::new(0.11, 1.62), 0.025, 0.0, FRAC_PI_2, 4));
    post.extend([Vec2::new(0.085, 1.66), Vec2::new(0.085, 1.72)]);
    let mut head = vec![Vec2::new(0.085, 1.72)];
    head.extend(arc_points(
        Vec2::new(0.0, 1.84),
        0.125,
        -FRAC_PI_2 * 0.82,
        FRAC_PI_2,
        12,
    ));
    let grain = move |a: f32, y: f32, r: f32| {
        let p = Vec3::new(a.cos() * r, y, a.sin() * r);
        r - 0.003
            * smooth(
                0.0,
                0.7,
                vn3(Vec3::new(p.x * 20.0, y * 2.0, p.z * 20.0), s32 ^ 9),
            )
    };
    let mut wood = lathe(&[&post, &head], 32, 0.15, &grain);
    swap_uv(&mut wood);
    let mut iron = Mesh::default();
    for (y, h, r) in [
        (0.3f32, 0.05f32, 0.152f32),
        (0.95, 0.05, 0.149),
        (1.25, 0.12, 0.165),
        (1.55, 0.05, 0.142),
    ] {
        let prof = [
            Vec2::new(0.13, y - h * 0.5),
            Vec2::new(r - 0.006, y - h * 0.5),
            Vec2::new(r, y - h * 0.5 + 0.006),
            Vec2::new(r, y + h * 0.5 - 0.006),
            Vec2::new(r - 0.006, y + h * 0.5),
            Vec2::new(0.13, y + h * 0.5),
        ];
        iron.merge(&lathe(&[&prof], 32, 0.15, &|_, _, r| r));
    }
    // Leather grip: a strap wound around the post.
    let turns = 7.0;
    let path: Vec<Vec3> = (0..=(turns * 24.0) as usize)
        .map(|i| {
            let t = i as f32 / (turns * 24.0);
            let a = t * turns * TAU;
            let y = 0.45 + 0.4 * t;
            Vec3::new(a.cos() * 0.152, y, -a.sin() * 0.152)
        })
        .collect();
    let mut leather = tube(&path, &|_| 0.016, 6, true);
    deform(&mut leather, |p| {
        let r = Vec2::new(p.x, p.z);
        let l = r.length().max(1e-4);
        // Flatten the strap against the post.
        let flat = 0.152 + (l - 0.152) * 0.45;
        Vec3::new(p.x / l * flat, p.y, p.z / l * flat)
    });
    (stone, wood, iron, leather)
}

/// The swinging arm along −Z from the pivot (origin): a squared beam with an iron ferrule
/// and pin. Returns (wood, iron).
pub fn arm_meshes(seed: u64) -> (Mesh, Mesh) {
    let mut rng = Rng::new(seed);
    let beam = beam_z(&mut rng, Vec3::new(0.09, 0.09, 1.0), 0.015);
    let wood = transformed(&beam, Mat4::from_translation(Vec3::new(0.0, 0.0, -0.5)));
    let mut iron = Mesh::default();
    let ferrule = rounded_box(
        Vec3::new(0.052, 0.052, 0.035),
        BoxOpts::round(0.008).bevel(2),
    );
    put(&mut iron, &ferrule, Vec3::new(0.0, 0.0, -0.95));
    put(&mut iron, &ferrule, Vec3::new(0.0, 0.0, -0.22));
    put(&mut iron, &stud_mesh(0.03), Vec3::new(0.0, 0.065, 0.0));
    (wood, iron)
}

/// The padded striking head of a sparring arm (origin at its center, axis along Z): stitched
/// leather panels.
pub fn tip_mesh() -> Mesh {
    let prof: Vec<Vec2> = (0..=16)
        .map(|k| {
            let t = -FRAC_PI_2 + PI * k as f32 / 16.0;
            Vec2::new(0.11 * t.cos(), 0.13 * t.sin())
        })
        .collect();
    let seams = |a: f32, y: f32, r: f32| {
        let k = (a / TAU * 6.0).rem_euclid(1.0);
        let seam = 1.0 - smooth(0.0, 0.06, k.min(1.0 - k));
        let band = 1.0 - smooth(0.0, 0.012, (y.abs() - 0.07).abs());
        r - 0.005 * seam - 0.004 * band
    };
    let m = lathe(&[&prof], 36, 0.11, &seams);
    transformed(&m, Mat4::from_rotation_x(FRAC_PI_2))
}

/// Brass: polished where handled, tarnished brown-green in the hollows (0.5 m repeat).
pub fn brass_set() -> TextureSet {
    bake_par(256, 0.5, |uv| {
        let t = fb(uv * 5.0, (5, 5), 4, 91);
        let scratch = smooth(
            0.85,
            0.98,
            1.0 - (pn(uv * Vec2::new(40.0, 3.0), (40, 3), 92) * 3.0).abs(),
        );
        let tarnish = smooth(0.0, 0.6, t);
        let a = mix(hex(0xc8a050), hex(0x6a5a32), tarnish * 0.7) * (1.0 + 0.1 * scratch);
        Texel {
            albedo: a,
            height: 0.0001 * t - 0.00005 * scratch,
            roughness: 0.28 + 0.35 * tarnish - 0.08 * scratch,
            metallic: 1.0 - 0.3 * tarnish,
            occlusion: 1.0,
        }
    })
}

// ============================================================================== kit

/// The arena's materials (textures baked once per world, keyed in [`ArtAssets`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mat {
    Sand,
    Stone,
    Wood,
    Iron,
    Brass,
    Barrel,
    Banner(Team),
    Coal,
    Weave,
    Leather,
    /// Unlit, blended fire ([`flame_image`]).
    Flame,
    /// Freshly raked sand, alpha-masked over the trampled sand.
    Raked,
    /// Wet sand and puddles: a film blended over the floor (vertex alpha), its texture running
    /// from damp sand, dark and a little glossy, through soaked sand to standing water, the
    /// dark bottom under a mirror ([`grounds::wet_set`]; the mesh picks the wetness per
    /// vertex).
    Damp,
    /// Still water in the troughs and casks: murky, faintly rippled and mirror smooth
    /// ([`grounds::water_set`]).
    Water,
    /// Straw and feathers: plain, colored per vertex, two-sided and a little translucent.
    Fiber,
    /// Tufts of straw lying on the sand: alpha-masked cards of [`grounds::straw_set`].
    Straw,
}

/// Emission of the glowing coal seams (the emissive image scales it down elsewhere).
const COAL_GLOW: f32 = 40_000.0;
const COAL_GLOW_KEY: &str = "npc/arena/coal/emissive";

impl Mat {
    /// Every material, the slowest to bake first.
    pub const ALL: [Mat; 17] = [
        Mat::Sand,
        Mat::Stone,
        Mat::Wood,
        Mat::Straw,
        Mat::Barrel,
        Mat::Banner(Team::Red),
        Mat::Banner(Team::Blue),
        Mat::Raked,
        Mat::Iron,
        Mat::Weave,
        Mat::Coal,
        Mat::Brass,
        Mat::Leather,
        Mat::Flame,
        Mat::Damp,
        Mat::Water,
        Mat::Fiber,
    ];

    /// Whether the material has a texture set (plain ones and the flame don't).
    fn textured(self) -> bool {
        !matches!(self, Mat::Flame | Mat::Fiber)
    }

    fn key(self) -> &'static str {
        match self {
            Mat::Sand => "arena/sand",
            Mat::Stone => "arena/stone",
            Mat::Wood => "arena/wood",
            Mat::Iron => "arena/iron",
            Mat::Brass => "arena/brass",
            Mat::Barrel => "arena/barrel",
            Mat::Banner(Team::Red) => "arena/banner_red",
            Mat::Banner(Team::Blue) => "arena/banner_blue",
            Mat::Coal => "arena/coal",
            Mat::Weave => "arena/mat",
            Mat::Leather => "arena/leather",
            Mat::Flame => "arena/flame",
            Mat::Raked => "arena/raked",
            Mat::Damp => "arena/damp",
            Mat::Water => "arena/water",
            Mat::Fiber => "arena/fiber",
            Mat::Straw => "arena/straw_tufts",
        }
    }

    fn base(self) -> StandardMaterial {
        let tiled = |meters: f32| {
            StandardMaterial {
                base_color: [1.0; 4],
                ..Default::default()
            }
            .with_uv_scale(1.0 / meters, 1.0 / meters)
        };
        match self {
            Mat::Sand => tiled(span::SAND),
            Mat::Stone => tiled(span::STONE),
            Mat::Wood => tiled(span::WOOD),
            Mat::Iron => tiled(span::IRON),
            Mat::Brass => tiled(0.5),
            Mat::Barrel => tiled(1.0),
            Mat::Banner(_) => {
                let mut m = tiled(1.0).with_subsurface(0.4).with_wind_sway(0.12);
                m.double_sided = true;
                m
            }
            Mat::Coal => tiled(span::COAL).with_emissive([1.0, 1.0, 1.0], COAL_GLOW),
            Mat::Weave => tiled(span::MAT),
            Mat::Leather => tiled(span::LEATHER),
            Mat::Flame => StandardMaterial {
                base_color: [0.0, 0.0, 0.0, 1.0],
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                double_sided: true,
                ..Default::default()
            }
            .with_emissive([1.0; 3], FLAME_GLOW),
            Mat::Raked => StandardMaterial {
                alpha_mode: AlphaMode::Mask(0.5),
                ..tiled(span::RAKED)
            },
            Mat::Straw => StandardMaterial {
                alpha_mode: AlphaMode::Mask(0.5),
                ..tiled(1.0)
            }
            .with_subsurface(0.25),
            // Water's reflectance is lower, but in full sun its sky reflection is faint next to
            // the sunlit sand: more of it makes the puddles read at a glance.
            Mat::Damp => StandardMaterial {
                alpha_mode: AlphaMode::Blend,
                reflectance: 1.4,
                receives_wetness: false,
                ..tiled(1.0)
            },
            Mat::Water => StandardMaterial {
                receives_wetness: false,
                ..tiled(1.0)
            },
            Mat::Fiber => {
                let mut m = StandardMaterial {
                    base_color: [1.0; 4],
                    perceptual_roughness: 0.62,
                    ..Default::default()
                }
                .with_subsurface(0.35);
                m.double_sided = true;
                m
            }
        }
    }

    fn bake(self) -> Baked {
        let set = |set: TextureSet| Baked {
            set: Some(set),
            glow: None,
        };
        match self {
            Mat::Sand => set(sand_set()),
            Mat::Stone => set(stone_set()),
            Mat::Wood => set(wood_set()),
            Mat::Iron => set(iron_set()),
            Mat::Brass => set(brass_set()),
            Mat::Barrel => set(barrel_set()),
            Mat::Banner(t) => set(banner_set(t)),
            Mat::Coal => {
                let (set, glow) = coal_set();
                Baked {
                    set: Some(set),
                    glow: Some(glow),
                }
            }
            Mat::Weave => set(mat_set()),
            Mat::Leather => set(leather_set()),
            Mat::Flame => Baked {
                set: None,
                glow: Some(flame_image()),
            },
            Mat::Raked => set(grounds::raked_set()),
            Mat::Straw => set(grounds::straw_set()),
            Mat::Water => set(grounds::water_set()),
            Mat::Damp => set(grounds::wet_set()),
            Mat::Fiber => Baked {
                set: None,
                glow: None,
            },
        }
    }

    /// The key of the material's emission image, if it glows.
    fn glow_key(self) -> Option<&'static str> {
        match self {
            Mat::Coal => Some(COAL_GLOW_KEY),
            Mat::Flame => Some(FLAME_KEY),
            _ => None,
        }
    }

    fn loaded(self, art: &ArtAssets) -> bool {
        let Some(images) = art.images.as_ref() else {
            return false;
        };
        (!self.textured()
            || images
                .loaded_by_key(&format!("npc/{}/orm", self.key()))
                .is_some())
            && self
                .glow_key()
                .is_none_or(|k| images.loaded_by_key(k).is_some())
    }
}

/// The material `m` (its textures baked on first use, or taken from what this process baked
/// for another world). `None` without the renderer.
pub fn material(art: &mut ArtAssets, m: Mat) -> Option<MaterialRef> {
    if !m.loaded(art) {
        store(art, m, &baked(m))?;
    }
    let images = art.images.as_mut()?;
    let mut s = m.base();
    if m.textured() {
        let k = format!("npc/{}", m.key());
        let orm = images.loaded_by_key(&format!("{k}/orm"))?;
        s.base_color_texture = Some(images.loaded_by_key(&format!("{k}/albedo"))?);
        s.normal_texture = Some(images.loaded_by_key(&format!("{k}/normal"))?);
        s.metallic_roughness_texture = Some(orm);
        s.occlusion_texture = Some(orm);
        // The ORM map carries roughness and metalness; the factors scale it.
        s.perceptual_roughness = 1.0;
        s.metallic = 1.0;
    }
    if let Some(k) = m.glow_key() {
        let glow = images.loaded_by_key(k)?;
        if m == Mat::Flame {
            // The flame image's alpha is the flame's shape.
            s.base_color_texture = Some(glow);
        }
        s.emissive_texture = Some(glow);
    }
    Some(MaterialRef::Inline(s))
}

/// `m` baked, from the process cache when an earlier world baked it.
fn baked(m: Mat) -> Arc<Baked> {
    if let Some(b) = lock().textures.get(&m) {
        return b.clone();
    }
    let b = Arc::new(m.bake());
    lock().textures.insert(m, b.clone());
    b
}

/// Put `m`'s baked images in the image store.
fn store(art: &mut ArtAssets, m: Mat, b: &Baked) -> Option<()> {
    let images = art.images.as_mut()?;
    if let Some(set) = &b.set {
        let k = format!("npc/{}", m.key());
        images.insert_keyed(&format!("{k}/albedo"), set.albedo.clone());
        images.insert_keyed(&format!("{k}/normal"), set.normal.clone());
        images.insert_keyed(&format!("{k}/orm"), set.orm.clone());
    }
    if let (Some(glow), Some(k)) = (&b.glow, m.glow_key()) {
        images.insert_keyed(k, glow.clone());
    }
    Some(())
}

/// Meshes stored under `keys` (built together by `make` when any is missing). `None` without
/// the renderer.
pub fn meshes<const N: usize>(
    art: &mut ArtAssets,
    keys: [&str; N],
    make: impl FnOnce() -> [Mesh; N],
) -> Option<[MeshRef; N]> {
    let store = art.meshes.as_mut()?;
    let full: Vec<String> = keys.iter().map(|k| format!("npc/{k}")).collect();
    if full.iter().any(|k| store.loaded_by_key(k).is_none()) {
        let built = make();
        for (k, m) in full.iter().zip(built) {
            store.insert_keyed(k, m);
        }
    }
    let handles: Vec<MeshRef> = full
        .iter()
        .map(|k| store.loaded_by_key(k).map(MeshRef::Handle))
        .collect::<Option<_>>()?;
    handles.try_into().ok()
}

/// What `demo::spawn_demo` laid out, for [`dress`] to put art on. Pieces are gameplay
/// entities whose colliders stay as they are; the art goes on as children.
#[derive(Debug, Clone, Default)]
pub struct ArenaLayout {
    pub origin: DVec3,
    pub pieces: Vec<(Entity, Piece)>,
    /// Brazier spots on the floor (world).
    pub braziers: Vec<DVec3>,
    /// Banners on the top walkway: angle (radians, from +X toward +Z) and team.
    pub banners: Vec<(f64, Team)>,
    /// Sparring mat centers on the floor (world).
    pub mats: Vec<DVec3>,
    /// Center of the duel ring on the floor (world).
    pub duel_ring: DVec3,
    /// Where fighters trample the sand: center (x, z relative to the origin) and radius.
    pub trample: Vec<(Vec2, f32)>,
}

/// Which art dresses a gameplay entity (its origin is the collider's center).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Piece {
    /// A first-tier stand collider: the stand segments cover it, nothing to add.
    Podium,
    /// A 7 m column; the variant picks the mesh.
    Pillar(u32),
    StoneWall(u32),
    TimberWall,
    Crates(u32),
    Barrel(u32),
    /// Half extents.
    Platform(Vec3),
    /// Index (0 = lowest) and half extents.
    Step(u32, Vec3),
    /// Half extents, pitch about X, and the floor's height below the origin.
    Ramp(Vec3, f32, f32),
    WallRun(Vec3),
}

/// One mesh job: the keys it fills, what the meshes depend on (for the process cache) and
/// the builder.
struct Spec {
    keys: Vec<String>,
    tag: String,
    make: Box<dyn Fn() -> Vec<Mesh> + Send + Sync>,
}

fn spec<const N: usize>(
    keys: [&str; N],
    make: impl Fn() -> [Mesh; N] + Send + Sync + 'static,
) -> Spec {
    Spec {
        keys: keys.iter().map(|k| k.to_string()).collect(),
        tag: keys.join(" "),
        make: Box::new(move || make().into_iter().collect()),
    }
}

impl Spec {
    /// Meshes that depend on the layout (`params`) are cached per layout.
    fn tagged(mut self, params: impl std::fmt::Debug) -> Spec {
        self.tag = format!("{} {params:?}", self.tag);
        self
    }
}

const STAND_VARIANTS: u32 = 4;

fn specs(layout: &ArenaLayout, g: &Arc<Grounds>) -> Vec<Spec> {
    let mut v = Vec::new();
    for k in 0..STAND_VARIANTS {
        v.push(spec([&format!("arena/stand{k}")], move || {
            [stand_segment(1000 + k as u64 * 77)]
        }));
    }
    v.push(spec(
        ["arena/gate_stone", "arena/gate_wood", "arena/gate_iron"],
        || {
            let (s, w, i) = entrance_meshes(4242);
            [s, w, i]
        },
    ));
    for k in 0..2 {
        v.push(spec([&format!("arena/pillar{k}")], move || {
            [pillar_mesh(k)]
        }));
        v.push(spec([&format!("arena/wall_stone{k}")], move || {
            [stone_wall_mesh(300 + k as u64)]
        }));
        v.push(spec(
            [
                &format!("arena/crates{k}_wood"),
                &format!("arena/crates{k}_iron"),
            ],
            move || {
                let (w, i) = crate_stack_meshes(500 + k as u64);
                [w, i]
            },
        ));
        v.push(spec([&format!("arena/barrel{k}")], move || {
            [barrel_mesh(600 + k as u64)]
        }));
        v.push(spec([&format!("arena/banner_cloth{k}")], move || {
            [banner_cloth_mesh(800 + k as u64)]
        }));
        v.push(spec([&format!("arena/rubble{k}")], move || {
            [rubble_mesh(k)]
        }));
        v.push(spec([&format!("arena/flame{k}")], move || {
            [flame_mesh(990 + k as u64)]
        }));
    }
    let gc = g.clone();
    v.push(
        spec(["arena/floor", "arena/floor_raked"], move || {
            let (b, r) = gc.floor_meshes();
            [b, r]
        })
        .tagged(g.tag),
    );
    let gc = g.clone();
    v.push(
        spec(["arena/drifts", "arena/tracks"], move || {
            [gc.drift_mesh(), gc.track_mesh()]
        })
        .tagged(g.tag),
    );
    let gc = g.clone();
    v.push(spec(["arena/wet"], move || [gc.wet_mesh()]).tagged(g.tag));
    let gc = g.clone();
    v.push(
        spec(
            [
                "arena/straw",
                "arena/fiber",
                "arena/litter_wood",
                "arena/litter_iron",
            ],
            move || {
                let (s, f, w, i) = gc.litter_meshes();
                [s, f, w, i]
            },
        )
        .tagged(g.tag),
    );
    for k in 0..2u64 {
        v.push(spec(
            [
                &format!("arena/trough{k}_stone"),
                &format!("arena/trough{k}_iron"),
                &format!("arena/trough{k}_water"),
            ],
            move || {
                let (s, i, w) = grounds::trough_meshes(1100 + k);
                [s, i, w]
            },
        ));
        v.push(spec(
            [
                &format!("arena/cask{k}_wood"),
                &format!("arena/cask{k}_iron"),
                &format!("arena/cask{k}_water"),
            ],
            move || {
                let (w, i, water) = grounds::cask_meshes(1110 + k);
                [w, i, water]
            },
        ));
        v.push(spec(
            [
                &format!("arena/rack{k}_wood"),
                &format!("arena/rack{k}_iron"),
                &format!("arena/rack{k}_leather"),
            ],
            move || {
                let (w, i, l) = grounds::rack_meshes(1120 + k);
                [w, i, l]
            },
        ));
    }
    v.push(spec(
        ["arena/wall_timber_wood", "arena/wall_timber_iron"],
        || {
            let (w, i) = timber_wall_meshes(350);
            [w, i]
        },
    ));
    for &(_, p) in &layout.pieces {
        match p {
            Piece::Platform(half) => {
                v.push(spec(["arena/platform"], move || [platform_mesh(half, 900)]).tagged(half))
            }
            Piece::Step(k, half) => v.push(
                spec([&format!("arena/step{k}")], move || {
                    [step_mesh(half, 910 + k as u64)]
                })
                .tagged(half),
            ),
            Piece::Ramp(half, tilt, ground) => v.push(
                spec(["arena/ramp_wood", "arena/ramp_iron"], move || {
                    let (w, i) = ramp_meshes(half, tilt, ground, 920);
                    [w, i]
                })
                .tagged((half, tilt, ground)),
            ),
            Piece::WallRun(half) => {
                v.push(spec(["arena/wall_run"], move || [wall_run_mesh(half, 930)]).tagged(half))
            }
            _ => {}
        }
    }
    v.push(spec(
        [
            "arena/brazier_stone",
            "arena/brazier_iron",
            "arena/brazier_coal",
        ],
        || {
            let (s, i, c) = brazier_meshes(940);
            [s, i, c]
        },
    ));
    v.push(spec(["arena/banner_iron", "arena/banner_brass"], || {
        let (i, b) = banner_pole_meshes();
        [i, b]
    }));
    v.push(spec(["arena/ring_stone", "arena/ring_brass"], || {
        let (s, b) = ring_meshes(950);
        [s, b]
    }));
    v.push(spec(["arena/mat_weave", "arena/mat_leather"], || {
        let (m, l) = mat_meshes(960);
        [m, l]
    }));
    v
}

/// Everything the arena needs, loaded: meshes by key (with triangle counts) and materials.
struct Kit {
    meshes: HashMap<String, (MeshRef, usize)>,
    mats: HashMap<Mat, MaterialRef>,
}

enum Work<'a> {
    Tex(Mat),
    Mesh(&'a Spec),
}

enum Done<'a> {
    Tex(Mat, Arc<Baked>),
    Mesh(&'a Spec, Vec<Arc<Mesh>>),
}

impl Kit {
    /// Build whatever is missing on all cores (or take it from what this process built for
    /// another world), store it, and collect the handles.
    fn load(art: &mut ArtAssets, layout: &ArenaLayout, g: &Arc<Grounds>) -> Option<Kit> {
        let specs = specs(layout, g);
        let missing_mats: Vec<Mat> = Mat::ALL.into_iter().filter(|&m| !m.loaded(art)).collect();
        let missing_specs: Vec<&Spec> = {
            let store = art.meshes.as_ref()?;
            specs
                .iter()
                .filter(|s| {
                    s.keys
                        .iter()
                        .any(|k| store.loaded_by_key(&format!("npc/{k}")).is_none())
                })
                .collect()
        };
        let mut textures: Vec<(Mat, Arc<Baked>)> = Vec::new();
        let mut meshes: Vec<(String, Arc<Mesh>)> = Vec::new();
        let mut work: Vec<Work> = Vec::new();
        {
            let cache = lock();
            for m in missing_mats {
                match cache.textures.get(&m) {
                    Some(b) => textures.push((m, b.clone())),
                    None => work.push(Work::Tex(m)),
                }
            }
            for s in missing_specs {
                match cache.meshes.get(&s.tag) {
                    Some(built) => meshes.extend(s.keys.iter().cloned().zip(built.iter().cloned())),
                    None => work.push(Work::Mesh(s)),
                }
            }
        }
        // The slowest jobs (the big textures) are queued first.
        let done = par_map(work.len(), |i| match work[i] {
            Work::Tex(m) => Done::Tex(m, Arc::new(m.bake())),
            Work::Mesh(s) => Done::Mesh(s, (s.make)().into_iter().map(Arc::new).collect()),
        });
        {
            let mut cache = lock();
            for d in done {
                match d {
                    Done::Tex(m, b) => {
                        cache.textures.insert(m, b.clone());
                        textures.push((m, b));
                    }
                    Done::Mesh(s, built) => {
                        meshes.extend(s.keys.iter().cloned().zip(built.iter().cloned()));
                        cache.meshes.insert(s.tag.clone(), built);
                    }
                }
            }
        }
        for (m, b) in textures {
            store(art, m, &b)?;
        }
        let store = art.meshes.as_mut()?;
        for (k, m) in meshes {
            store.insert_keyed(&format!("npc/{k}"), (*m).clone());
        }
        let mut kit = Kit {
            meshes: HashMap::new(),
            mats: HashMap::new(),
        };
        for s in &specs {
            for k in &s.keys {
                let h = store.loaded_by_key(&format!("npc/{k}"))?;
                let tris = store.get(h).map(|m| m.triangle_count()).unwrap_or(0);
                kit.meshes.insert(k.clone(), (MeshRef::Handle(h), tris));
            }
        }
        for m in Mat::ALL {
            kit.mats.insert(m, material(art, m)?);
        }
        Some(kit)
    }
}

/// A material's baked images: its texture set (none for the flame) and its emission image.
struct Baked {
    set: Option<TextureSet>,
    glow: Option<Image>,
}

/// What this process has built, so another world (a test, a reloaded level) gets the arena
/// without building it again. Meshes are keyed by their spec's tag.
#[derive(Default)]
struct Cache {
    textures: HashMap<Mat, Arc<Baked>>,
    meshes: HashMap<String, Vec<Arc<Mesh>>>,
}

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Spawns art entities and counts the triangles it places.
struct Dresser<'k> {
    kit: &'k Kit,
    triangles: usize,
}

impl Dresser<'_> {
    fn part(&mut self, world: &mut World, parent: Entity, local: Transform, key: &str, mat: Mat) {
        self.part_ex(world, parent, local, key, mat, true);
    }

    /// Like [`Dresser::part`], optionally without shadows, returning the new entity.
    fn part_ex(
        &mut self,
        world: &mut World,
        parent: Entity,
        local: Transform,
        key: &str,
        mat: Mat,
        shadows: bool,
    ) -> Option<Entity> {
        let (mesh, tris) = self.kit.meshes.get(key).cloned()?;
        let material = self.kit.mats.get(&mat).cloned()?;
        self.triangles += tris;
        let mut r = MeshRenderer::new(mesh, material);
        r.cast_shadows = shadows;
        Some(world.spawn((local, r, ChildOf(parent))).id())
    }
}

/// Height of a brazier's fire (on its coals) above the floor, meters.
pub const FIRE_HEIGHT: f64 = 1.14;

/// The [`Flicker`] seed of brazier `n` (in `ArenaLayout::braziers` order).
pub fn fire_seed(n: usize) -> f32 {
    n as f32 * 1.37 + 0.4
}

fn yaw_at(a: f64) -> Quat {
    Quat::from_rotation_y(-(a as f32) + FRAC_PI_2)
}

/// Put the art on an arena laid out by `demo::spawn_demo` (see the module docs). Records
/// `npc.arena_build_ms` and `npc.arena_triangles` in [`Diagnostics`] when present.
pub fn dress(world: &mut World, layout: &ArenaLayout) {
    let start = std::time::Instant::now();
    let grounds = Arc::new(Grounds::plan(world, layout));
    let mut state = SystemState::<ArtAssets>::new(world);
    let kit = match state.get_mut(world) {
        Ok(mut art) if art.available() => Kit::load(&mut art, layout, &grounds),
        _ => None,
    };
    let Some(kit) = kit else {
        fallback(world, layout);
        return;
    };
    let mut d = Dresser {
        kit: &kit,
        triangles: 0,
    };
    let origin = layout.origin;
    let id = Transform::default();
    // The floor: trampled sand, the raked layer, drifts, ruts and drag marks, wet sand and
    // puddles (one blended film, so the puddles draw over the damp sand), and litter.
    for (key, mat, shadows) in [
        ("arena/floor", Mat::Sand, false),
        ("arena/floor_raked", Mat::Raked, false),
        ("arena/drifts", Mat::Sand, false),
        ("arena/tracks", Mat::Sand, false),
        ("arena/wet", Mat::Damp, false),
        // Litter is too fine for the shadow maps (it would only shadow itself).
        ("arena/straw", Mat::Straw, false),
        ("arena/fiber", Mat::Fiber, false),
        ("arena/litter_wood", Mat::Wood, false),
        ("arena/litter_iron", Mat::Iron, false),
    ] {
        let root = world.spawn(Transform::from_translation(origin)).id();
        let _ = d.part_ex(world, root, id, key, mat, shadows);
    }
    // Troughs, casks and racks along the walls: decoration with no colliders.
    for p in &grounds.props {
        let at = origin + DVec3::new(p.at.x as f64, grounds.ground(p.at) as f64, p.at.y as f64);
        let root = world
            .spawn(Transform::from_translation(at).with_rotation(yaw_at(p.facing as f64)))
            .id();
        let parts: [(String, Mat); 3] = match p.prop {
            Prop::Trough(k) => [
                (format!("arena/trough{k}_stone"), Mat::Stone),
                (format!("arena/trough{k}_iron"), Mat::Iron),
                (format!("arena/trough{k}_water"), Mat::Water),
            ],
            Prop::Cask(k) => [
                (format!("arena/cask{k}_wood"), Mat::Wood),
                (format!("arena/cask{k}_iron"), Mat::Iron),
                (format!("arena/cask{k}_water"), Mat::Water),
            ],
            Prop::Rack(k) => [
                (format!("arena/rack{k}_wood"), Mat::Wood),
                (format!("arena/rack{k}_iron"), Mat::Iron),
                (format!("arena/rack{k}_leather"), Mat::Leather),
            ],
        };
        for (key, mat) in parts {
            d.part(world, root, id, &key, mat);
        }
    }
    // Stands: 36 sectors of 10°, gates at 0°, 90°, 180° and 270°.
    for i in 0..36u32 {
        let a = i as f64 / 36.0 * std::f64::consts::TAU;
        let root = world
            .spawn(Transform::from_translation(origin).with_rotation(yaw_at(a)))
            .id();
        if i % 9 == 0 {
            d.part(world, root, id, "arena/gate_stone", Mat::Stone);
            d.part(world, root, id, "arena/gate_wood", Mat::Wood);
            d.part(world, root, id, "arena/gate_iron", Mat::Iron);
        } else {
            let k = (i * 7 + i / 3) % STAND_VARIANTS;
            d.part(world, root, id, &format!("arena/stand{k}"), Mat::Stone);
        }
    }
    for &(e, piece) in &layout.pieces {
        match piece {
            Piece::Podium => {}
            Piece::Pillar(k) => d.part(world, e, id, &format!("arena/pillar{}", k % 2), Mat::Stone),
            Piece::StoneWall(k) => d.part(
                world,
                e,
                id,
                &format!("arena/wall_stone{}", k % 2),
                Mat::Stone,
            ),
            Piece::TimberWall => {
                d.part(world, e, id, "arena/wall_timber_wood", Mat::Wood);
                d.part(world, e, id, "arena/wall_timber_iron", Mat::Iron);
            }
            Piece::Crates(k) => {
                d.part(
                    world,
                    e,
                    id,
                    &format!("arena/crates{}_wood", k % 2),
                    Mat::Wood,
                );
                d.part(
                    world,
                    e,
                    id,
                    &format!("arena/crates{}_iron", k % 2),
                    Mat::Iron,
                );
            }
            Piece::Barrel(k) => {
                d.part(world, e, id, &format!("arena/barrel{}", k % 2), Mat::Barrel)
            }
            Piece::Platform(_) => d.part(world, e, id, "arena/platform", Mat::Stone),
            Piece::Step(k, _) => d.part(world, e, id, &format!("arena/step{k}"), Mat::Stone),
            Piece::Ramp(..) => {
                d.part(world, e, id, "arena/ramp_wood", Mat::Wood);
                d.part(world, e, id, "arena/ramp_iron", Mat::Iron);
            }
            Piece::WallRun(_) => d.part(world, e, id, "arena/wall_run", Mat::Stone),
        }
    }
    // Rubble at the foot of the stone walls, on whichever side and end is clearest of the
    // other pieces (barrels stand close to some walls).
    let obstacles: Vec<(Entity, DVec3, f64)> = layout
        .pieces
        .iter()
        .filter_map(|&(e, _)| {
            let at = world.get::<Transform>(e)?.translation;
            let r = match &world.get::<Collider>(e)?.shape {
                ColliderShape::Cuboid { half_extents } => {
                    Vec2::new(half_extents.x, half_extents.z).length()
                }
                ColliderShape::Cylinder { radius, .. } => *radius,
                _ => return None,
            };
            Some((e, at, r as f64))
        })
        .chain(
            layout
                .braziers
                .iter()
                .chain(&layout.mats)
                .map(|&at| (Entity::PLACEHOLDER, at, 1.0)),
        )
        .collect();
    for &(e, piece) in &layout.pieces {
        let (Piece::StoneWall(_), Some(&wall)) = (piece, world.get::<Transform>(e)) else {
            continue;
        };
        let mut best = (f64::MIN, 0, wall.rotation);
        for variant in 0..2u32 {
            let side = if variant == 0 { 1.0 } else { -1.0 };
            for turn in [0.0, PI] {
                let rot = wall.rotation * Quat::from_rotation_y(turn);
                let clear = [(0.85, 0.6), (0.2, 0.55), (1.8, -0.1), (-0.6, 0.5)]
                    .iter()
                    .map(|&(x, z)| {
                        wall.translation + (rot * Vec3::new(side * x, 0.0, z)).as_dvec3()
                    })
                    .flat_map(|p| {
                        obstacles.iter().filter(|o| o.0 != e).map(move |o| {
                            let d = p - o.1;
                            (d.x * d.x + d.z * d.z).sqrt() - o.2
                        })
                    })
                    .fold(f64::MAX, f64::min);
                if clear > best.0 {
                    best = (clear, variant, rot);
                }
            }
        }
        let root = world
            .spawn(Transform::from_translation(wall.translation).with_rotation(best.2))
            .id();
        d.part(
            world,
            root,
            id,
            &format!("arena/rubble{}", best.1),
            Mat::Stone,
        );
    }
    // Braziers: the fire flickers in step with the brazier's light (`demo` gives it the same
    // seed).
    for (n, &at) in layout.braziers.iter().enumerate() {
        let root = world.spawn(Transform::from_translation(at)).id();
        d.part(world, root, id, "arena/brazier_stone", Mat::Stone);
        d.part(world, root, id, "arena/brazier_iron", Mat::Iron);
        let _ = d.part_ex(world, root, id, "arena/brazier_coal", Mat::Coal, false);
        let fire = Transform::from_translation(DVec3::Y * FIRE_HEIGHT);
        let key = format!("arena/flame{}", n % 2);
        if let Some(flame) = d.part_ex(world, root, fire, &key, Mat::Flame, false) {
            world.entity_mut(flame).insert(Flicker {
                seed: fire_seed(n),
                lumens: 0.0,
            });
        }
    }
    for (n, &(a, team)) in layout.banners.iter().enumerate() {
        let at = origin + DVec3::new(a.cos() * 47.0, 3.2, a.sin() * 47.0);
        let root = world
            .spawn(Transform::from_translation(at).with_rotation(yaw_at(a)))
            .id();
        d.part(world, root, id, "arena/banner_iron", Mat::Iron);
        d.part(world, root, id, "arena/banner_brass", Mat::Brass);
        let cloth = Transform::default().with_rotation(Quat::from_rotation_z(PI));
        d.part(
            world,
            root,
            cloth,
            &format!("arena/banner_cloth{}", n % 2),
            Mat::Banner(team),
        );
    }
    let ring = world
        .spawn(Transform::from_translation(layout.duel_ring))
        .id();
    let _ = d.part_ex(world, ring, id, "arena/ring_stone", Mat::Stone, false);
    let _ = d.part_ex(world, ring, id, "arena/ring_brass", Mat::Brass, false);
    for &at in &layout.mats {
        let root = world.spawn(Transform::from_translation(at)).id();
        let _ = d.part_ex(world, root, id, "arena/mat_weave", Mat::Weave, false);
        let _ = d.part_ex(world, root, id, "arena/mat_leather", Mat::Leather, false);
    }
    let triangles = d.triangles;
    if let Some(mut diag) = world.get_resource_mut::<Diagnostics>() {
        diag.set("npc.arena_build_ms", start.elapsed().as_secs_f64() * 1000.0);
        diag.set("npc.arena_triangles", triangles as f64);
    }
}

/// Without the renderer: plain shapes on the gameplay pieces and a floor disc.
fn fallback(world: &mut World, layout: &ArenaLayout) {
    let stone = StandardMaterial::hex(0x9a9184).with_roughness(0.92);
    world.spawn((
        Transform::from_translation(layout.origin + DVec3::Y * 0.02),
        MeshRenderer::new(
            Shape::Cylinder {
                radius: 40.0,
                height: 0.04,
                segments: 96,
            },
            StandardMaterial::hex(0xa48d66).with_roughness(0.97),
        ),
    ));
    for &(e, piece) in &layout.pieces {
        let Some(c) = world.get::<Collider>(e) else {
            continue;
        };
        let shape = match c.shape {
            ColliderShape::Cuboid { half_extents } => Shape::Cuboid {
                size: (half_extents * 2.0).to_array(),
            },
            ColliderShape::Cylinder {
                radius,
                half_height,
            } => Shape::Cylinder {
                radius,
                height: half_height * 2.0,
                segments: 16,
            },
            _ => continue,
        };
        let material = match piece {
            Piece::Barrel(_) => StandardMaterial::hex(0xc0281e)
                .with_roughness(0.45)
                .with_metallic(0.4),
            Piece::Crates(_) | Piece::TimberWall | Piece::Ramp(..) => {
                StandardMaterial::hex(0x8a6a44).with_roughness(0.8)
            }
            _ => stone,
        };
        world
            .entity_mut(e)
            .insert(MeshRenderer::new(shape, material));
    }
}

/// Meshes and materials of a sparring post, built once per world. `None` without the
/// renderer.
pub struct PostKit {
    /// Static body pieces (on the post).
    pub body: Vec<(MeshRef, MaterialRef)>,
    /// Arm pieces (on the swinging arm).
    pub arm: Vec<(MeshRef, MaterialRef)>,
    pub tip: MeshRef,
    pub tip_cold: MaterialRef,
    pub tip_hot: MaterialRef,
}

impl PostKit {
    pub fn load(art: &mut ArtAssets) -> Option<PostKit> {
        let [stone, wood, iron, leather] = meshes(
            art,
            [
                "arena/post_stone",
                "arena/post_wood",
                "arena/post_iron",
                "arena/post_leather",
            ],
            || {
                let (s, w, i, l) = dummy_meshes(970);
                [s, w, i, l]
            },
        )?;
        let [arm_wood, arm_iron] = meshes(art, ["arena/arm_wood", "arena/arm_iron"], || {
            let (w, i) = arm_meshes(971);
            [w, i]
        })?;
        let [tip] = meshes(art, ["arena/tip"], || [tip_mesh()])?;
        let m_stone = material(art, Mat::Stone)?;
        let m_wood = material(art, Mat::Wood)?;
        let m_iron = material(art, Mat::Iron)?;
        let m_leather = material(art, Mat::Leather)?;
        let tip_hot = match &m_leather {
            MaterialRef::Inline(m) => MaterialRef::Inline(m.with_emissive([1.0, 0.35, 0.05], 6.0)),
            other => other.clone(),
        };
        Some(PostKit {
            body: vec![
                (stone, m_stone),
                (wood, m_wood.clone()),
                (iron, m_iron.clone()),
                (leather, m_leather.clone()),
            ],
            arm: vec![(arm_wood, m_wood), (arm_iron, m_iron)],
            tip,
            tip_cold: m_leather,
            tip_hot,
        })
    }
}
