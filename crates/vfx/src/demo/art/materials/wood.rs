//! Weathered wooden planks, and burning charcoal.

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::StandardMaterial;

use super::Generated;
use super::noise::{self, mix32, rand, sub, unit};
use super::textures::{Maps, Texel, Tile, hex, mix, mix3, plausible, relief, smooth, white};

/// One plank column: boards end to end along the grain (V).
struct Plank {
    x0: f32,
    w: f32,
    /// Butt joints along the plank (meters from the tile top), ascending.
    joints: Vec<f32>,
}

/// One board's look.
#[derive(Clone, Copy)]
struct Board {
    /// Pith offset across the board and depth below the face (m).
    pith: Vec2,
    taper: f32,
    rings_per_m: f32,
    weather: f32,
    tone: f32,
    cup: f32,
    knots: [Vec4; 2],
    checks: [Vec4; 2],
}

fn board(seed: u32, w: f32) -> Board {
    let r = |k: u32| rand(seed, k);
    let knot = |k: u32| {
        if r(k) < 0.55 {
            Vec4::new(r(k + 1) * w, r(k + 2), 0.005 + r(k + 3) * 0.01, 1.0)
        } else {
            Vec4::ZERO
        }
    };
    let check = |k: u32| {
        if r(k) < 0.6 {
            Vec4::new(
                (0.15 + 0.7 * r(k + 1)) * w,
                r(k + 2),
                0.08 + 0.35 * r(k + 3),
                1.0,
            )
        } else {
            Vec4::ZERO
        }
    };
    Board {
        pith: Vec2::new((r(1) - 0.5) * w * 1.6, 0.03 + r(2) * 0.12),
        taper: (r(3) - 0.5) * 0.06,
        rings_per_m: 220.0 + r(4) * 200.0,
        weather: 0.45 + r(5) * 0.55,
        tone: r(6),
        cup: (r(7) - 0.5) * 2.0,
        knots: [knot(10), knot(20)],
        checks: [check(30), check(40)],
    }
}

#[derive(Clone, Copy, Default)]
struct W {
    h: f32,
    late: f32,
    knot: f32,
    check: f32,
    seam: f32,
    nail: f32,
    rust: f32,
    weather: f32,
    tone: f32,
    v: f32,
}

/// Weathered planks running along V (the texture's height), with board seams, butt joints and
/// nails, grain from growth rings (cathedral figure where the log tapers), knots that bend
/// the grain, drying checks, raised latewood and silvered weathering. 0.6 m × 1.2 m per
/// tile.
pub fn wood(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 1024 >> shrink, 0.6);
    let size = tile.size;
    let seed = 0x00d0_0001;
    // Plank widths, normalized to fill the tile.
    let mut widths = Vec::new();
    let mut sum = 0.0;
    while sum < size.x - 0.06 {
        let w = 0.11 + 0.08 * rand(seed, widths.len() as u32);
        widths.push(w);
        sum += w;
    }
    let mut x0 = 0.0;
    let planks: Vec<Plank> = widths
        .iter()
        .enumerate()
        .map(|(j, w)| {
            let w = w / sum * size.x;
            let ps = sub(seed, 100 + j as u32);
            let mut joints = vec![rand(ps, 1) * size.y];
            if rand(ps, 2) < 0.35 {
                joints.push((joints[0] + (0.35 + 0.3 * rand(ps, 3)) * size.y) % size.y);
            }
            joints.sort_by(f32::total_cmp);
            let p = Plank { x0, w, joints };
            x0 += w;
            p
        })
        .collect();
    let boards: Vec<Vec<Board>> = planks
        .iter()
        .enumerate()
        .map(|(j, p)| {
            (0..p.joints.len())
                .map(|b| board(sub(seed, 1000 + (j * 8 + b) as u32), p.w))
                .collect()
        })
        .collect();
    // Grain waviness: long along the grain, short across it.
    let wave_period = [
        ((size.x / 0.03).round() as i32).max(1),
        ((size.y / 0.3).round() as i32).max(1),
    ];
    let fiber_period = [
        ((size.x / 0.0012).round() as i32).max(1),
        ((size.y / 0.03).round() as i32).max(1),
    ];
    let broad = tile.coarse(2, |p| {
        Vec4::new(
            noise::fbm(p, wave_period, 3, 0.5, sub(seed, 1)),
            tile.fbm(p, 0.2, 3, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.05, 3, 0.5, sub(seed, 3)),
            0.0,
        )
    });
    let gap = 0.0035;
    let px = tile.par(|p, x, y| {
        let b4 = broad.at(x, y);
        let m = p * size;
        let j = planks
            .iter()
            .position(|pl| m.x < pl.x0 + pl.w)
            .unwrap_or(planks.len() - 1);
        let pl = &planks[j];
        let u = m.x - pl.x0;
        // Which board: the last joint above (wrapping).
        let n = pl.joints.len();
        let k = pl.joints.iter().rposition(|&jy| jy <= m.y).unwrap_or(n - 1);
        let v = (m.y - pl.joints[k]).rem_euclid(size.y);
        let len = if n == 1 {
            size.y
        } else {
            (pl.joints[(k + 1) % n] - pl.joints[k]).rem_euclid(size.y)
        };
        let bd = &boards[j][k];
        let id = (j * 8 + k) as u32;
        // Growth rings: concentric around a pith below the face; the taper makes arches.
        let du = u - pl.w * 0.5 - bd.pith.x + b4.x * 0.004;
        let depth = bd.pith.y + (v - len * 0.5) * bd.taper;
        let mut phase = (du * du + depth * depth).sqrt() * bd.rings_per_m;
        // Knots bend the rings around them.
        let mut knot = 0.0f32;
        for kn in &bd.knots {
            if kn.w > 0.0 {
                let kc = Vec2::new(kn.x, kn.y * len);
                let d = Vec2::new(u - kc.x, (v - kc.y) * 0.3).length();
                phase += 4.0 * (-(d / (kn.z * 2.5)).powi(2)).exp();
                let dk = Vec2::new(u - kc.x, v - kc.y).length();
                if dk < kn.z * 1.3 {
                    let kring = (dk * 700.0).fract();
                    knot = knot.max(smooth(kn.z * 1.3, kn.z, dk) * (0.75 + 0.25 * kring));
                }
            }
        }
        let t = phase.fract();
        // Latewood: a narrow dense band ending each ring sharply.
        let late = smooth(0.55, 0.85, t) * (1.0 - smooth(0.93, 0.995, t));
        let fiber = noise::value(p, fiber_period, sub(seed, 4));
        let mut h = -(1.0 - late) * 0.00045 * bd.weather + (fiber - 0.5) * 0.00012;
        h += b4.z * 0.0003;
        // Cupping across the board.
        let across = u / pl.w - 0.5;
        h += bd.cup * across * across * 0.002;
        h -= knot * 0.0004;
        // Drying checks along the grain.
        let mut check = 0.0f32;
        for c in &bd.checks {
            if c.w > 0.0 {
                let cv = c.y * len;
                let half = c.z * len * 0.5;
                let along = (v - cv).abs();
                if along < half {
                    let cu = c.x + b4.x * 0.006 + fiber * 0.0003;
                    let width = 0.0009 * (1.0 - (along / half).powi(2));
                    check = check.max(smooth(width, width * 0.3, (u - cu).abs()));
                }
            }
        }
        h -= check * 0.002;
        // Seams between planks and butt joints, with rounded worn edges.
        let ds = u.min(pl.w - u);
        let dj = v.min(len - v);
        let edge = ds.min(dj) - gap * 0.5;
        let round = 0.004;
        if edge < round {
            h -= 0.003 * (1.0 - (edge / round).max(0.0)).powi(2);
        }
        let seam = smooth(0.0, -0.0004, edge);
        h = mix(h, -0.008, seam);
        // Nails near board ends.
        let mut nail = 0.0f32;
        let mut rust = 0.0f32;
        for (nu, nv) in [(0.28, 0.025), (0.72, 0.025)] {
            for end in [nv, len - nv] {
                let d = Vec2::new(u - pl.w * nu, v - end).length();
                nail = nail.max(smooth(0.0045, 0.0035, d));
                rust = rust.max(smooth(0.02, 0.004, d + (fiber - 0.5) * 0.006));
            }
        }
        h = mix(h, 0.0002 - (unit(mix32(id)) * 0.0006), nail);
        W {
            h,
            late,
            knot,
            check,
            seam,
            nail,
            rust,
            weather: bd.weather,
            tone: bd.tone,
            v: v / len,
        }
    });
    let height: Vec<f32> = px.iter().map(|w| w.h).collect();
    let r = relief(&tile, &height, &[0.0015, 0.005, 0.02], 1.5, 0.0015);
    let texels = tile.par(|_, x, y| {
        let i = y * tile.w + x;
        let w = px[i];
        let b4 = broad.at(x, y);
        let bd = w.tone;
        let weather = w.weather;
        // Fresh wood under the weathering, and the silvered surface.
        let fresh = mix3(hex(0x9a7652), hex(0x8a6a4a), bd);
        let fresh_late = mix3(hex(0x5e4026), hex(0x6a4a30), bd);
        let gray = mix3(hex(0x8e877c), hex(0x9b9284), bd);
        let gray_late = hex(0x5c554c);
        let early = mix3(fresh, gray, weather);
        let late = mix3(fresh_late, gray_late, weather);
        let mut c = mix3(early, late, w.late);
        c *= 1.0 + b4.y * 0.12 + (white(i, 1) - 0.5) * 0.05;
        // Knots: dark, reddish, with their own rings.
        c = mix3(
            c,
            hex(0x4a3020) * mix(0.8, 1.1, w.knot),
            w.knot.min(1.0) * 0.9,
        );
        // Checks and grooves show fresher, darker wood.
        c = mix3(c, hex(0x3a2818), w.check * 0.9);
        // Grime toward seams and board ends; a little green algae in the damp.
        let near = smooth(0.12, 0.0, w.v.min(1.0 - w.v)) * 0.3;
        let damp = smooth(0.0, 0.5, b4.y) * 0.25;
        c = mix3(c, c * Vec3::new(0.62, 0.6, 0.55), near + damp * 0.5);
        c = mix3(c, hex(0x56603e), damp * smooth(0.85, 0.5, r.ao[i]));
        // Rust bleeding from nails.
        c = mix3(c, hex(0x5a3420), w.rust * 0.55);
        let nail_c = hex(0x2c2a28) * mix(0.8, 1.2, white(i, 2));
        c = mix3(c, nail_c, w.nail);
        c *= mix(0.45, 1.0, r.ao[i]);
        c = mix3(c, hex(0x1e1712), w.seam);
        let mut rough =
            0.82 - w.late * 0.08 + w.check * 0.1 + near * 0.05 + (white(i, 3) - 0.5) * 0.06;
        rough = mix(rough, 0.55, w.nail);
        let mut t = Texel::new(plausible(c), rough, r.ao[i]);
        t.metal = w.nail * 0.7;
        t
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}

/// Burning charcoal: black lumps crazed into small blocks (the alligator checking of charred
/// wood), with hot orange-yellow fissures, dull red faces where the heat is deepest, and pale
/// ash on the cooler blocks. The emissive texture carries the glow. 0.3 m per tile.
pub fn coals(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.3);
    let seed = 0x00d0_0c0a;
    #[derive(Clone, Copy, Default)]
    struct P {
        h: f32,
        crack: f32,
        heat: f32,
        ash: f32,
        block: f32,
    }
    let px = tile.par(|p, _, _| {
        let q = tile.warp(p, 0.08, 0.004, 2, sub(seed, 1));
        // Blocks a few centimeters across.
        let v = tile.voronoi_edge(q, 0.03, 0.85, sub(seed, 2));
        let fine = tile.voronoi_edge(q, 0.012, 0.9, sub(seed, 3));
        let crack = smooth(0.045, 0.0, v.edge).max(smooth(0.03, 0.0, fine.edge) * 0.4);
        // Each block domes a little and sits at its own height.
        let dome = (1.0 - v.f1 * 1.6).max(0.0);
        let h = 0.002 * dome + 0.0015 * rand(v.id, 4) - 0.004 * crack
            + 0.0006 * tile.fbm(p, 0.01, 3, 0.5, sub(seed, 5));
        // Heat comes in broad patches; ash settles on the cooler blocks.
        let heat = tile.fbm(p, 0.12, 3, 0.5, sub(seed, 6)) * 0.8 + 0.5;
        let ash = smooth(0.7, 0.9, rand(v.id, 7)) * smooth(0.45, 0.3, heat);
        P {
            h,
            crack,
            heat,
            ash,
            // Blocks glow unevenly.
            block: mix(0.35, 1.0, rand(v.id, 8)),
        }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.002, 0.008], 1.5, 0.003);
    let texels = tile.par_idx(|i| {
        let v = px[i];
        let ao = r.ao[i];
        let charcoal = hex(0x141210) * mix(0.8, 1.2, white(i, 9));
        let mut c = mix3(charcoal, hex(0x6e6963), v.ash * 0.7);
        c *= mix(0.6, 1.0, ao);
        // Glow: fissures burn yellow-orange, hot faces a dull red.
        let hot = smooth(0.35, 0.65, v.heat);
        let fissure = v.crack * mix(0.25, 1.0, hot);
        let face = smooth(0.55, 0.75, v.heat) * (1.0 - v.ash) * 0.35 * v.block;
        let glow = fissure + face;
        let color = mix3(
            Vec3::new(0.9, 0.12, 0.02),
            Vec3::new(1.0, 0.5, 0.12),
            fissure * hot,
        );
        Texel {
            albedo: plausible(c),
            rough: mix(0.85, 0.95, v.ash),
            metal: 0.0,
            ao,
            emissive: color * glow,
            alpha: 1.0,
        }
    });
    let maps = Maps::new(tile, r.normal, &texels, true, false);
    Generated::new(
        maps,
        StandardMaterial::default().with_emissive([1.0, 1.0, 1.0], 60.0),
    )
}
