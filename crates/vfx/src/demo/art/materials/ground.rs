//! Ground: wet mud with puddles, fresh snow, rippled sand, dirt, and clear water.

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::StandardMaterial;

use super::Generated;
use super::noise::{mix32, rand, sub, unit};
use super::textures::{
    Maps, Texel, Tile, add_slope, hex, mix, mix3, plausible, relief, smooth, white,
};

/// Thin straight bits (straw, twigs) scattered over cells about `cell_m` across: 0..1
/// coverage at `p`. `chance` of the cells hold one, up to `len` meters long.
fn strands(
    tile: &Tile,
    p: Vec2,
    cell_m: f32,
    chance: f32,
    len: f32,
    width_m: f32,
    seed: u32,
) -> f32 {
    let mut out = 0.0f32;
    tile.cells3(p, cell_m, seed, |id, r| {
        if unit(id) >= chance {
            return;
        }
        let a = unit(mix32(id ^ 1)) * std::f32::consts::PI;
        let dir = Vec2::new(a.cos(), a.sin());
        let along = r.dot(dir);
        let across = (r.x * dir.y - r.y * dir.x).abs();
        let half = len * 0.5 * (0.5 + 0.5 * unit(mix32(id ^ 2)));
        let w = width_m * (1.0 - 0.5 * (along / half).powi(2)).max(0.3);
        out = out.max(smooth(w, w * 0.4, across) * smooth(half, half * 0.85, along.abs()));
    });
    out
}

/// Wet mud: lumpy clods and ruts, glossy puddles filling the low spots, wet sheen at their
/// margins, embedded pebbles and straw. 1.5 m per tile.
pub fn mud(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 1.5);
    let seed = 0x0d0d_0001;
    let broad = tile.coarse(4, |p| {
        Vec4::new(
            tile.fbm(p, 0.5, 4, 0.5, sub(seed, 1)),
            tile.fbm(p, 0.15, 3, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.75, 2, 0.5, sub(seed, 3)),
            tile.fbm(p, 0.3, 3, 0.5, sub(seed, 4)),
        )
    });
    #[derive(Clone, Copy, Default)]
    struct M {
        h: f32,
        water: f32,
        pebble: f32,
        straw: f32,
        dry: f32,
    }
    let px = tile.par(|p, x, y| {
        let b = broad.at(x, y);
        let clods = tile.fbm(p, 0.04, 3, 0.55, sub(seed, 5));
        let mut h = b.x * 0.03 + b.y * 0.008 + clods * 0.003;
        h += (tile.value(p, 0.006, sub(seed, 6)) - 0.5) * 0.0006;
        // Pebbles: irregular, flattish stones half sunk in the mud.
        let pq = p + tile.off(Vec2::new(clods, b.y) * 0.006);
        let pv = tile.voronoi(pq, 0.03, 1.0, sub(seed, 7));
        let pr = 0.15 + 0.2 * unit(mix32(pv.id ^ 4));
        let pebble = if unit(pv.id) < 0.12 {
            let d = (pv.f1 / pr).min(1.0);
            smooth(1.0, 0.55, d) * (0.8 + 0.2 * (1.0 - d * d))
        } else {
            0.0
        };
        h += pebble * 0.003;
        let straw = strands(&tile, p, 0.08, 0.15, 0.06, 0.0035, sub(seed, 8));
        h += straw * 0.0008;
        // Puddles fill everything below the local water level.
        let level = -0.006 + b.z * 0.012;
        let depth = level - h;
        let water = smooth(-0.0003, 0.0008, depth);
        let dry = smooth(0.003, 0.016, h - level);
        M {
            h: h.max(level),
            water,
            pebble: pebble * (1.0 - water),
            straw: straw * (1.0 - water),
            dry,
        }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.004, 0.015, 0.06], 1.5, 0.004);
    let texels = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let b = broad.at(x, y);
        let ao = r.ao[i];
        let mut c = mix3(hex(0x45362a), hex(0x67543f), v.dry);
        c *= 1.0 + b.w * 0.15 + (tile.value(p, 0.01, sub(seed, 9)) - 0.5) * 0.14;
        c *= 1.0 + (white(i, 1) - 0.5) * 0.06;
        // Pebbles stay coated in mud except on their tops.
        let peb = mix3(
            hex(0x57504a),
            hex(0x5e4e3e),
            tile.value(p, 0.03, sub(seed, 10)),
        );
        c = mix3(c, peb, smooth(0.3, 0.9, v.pebble) * 0.7);
        c = mix3(c, hex(0x7a6a44), v.straw * 0.75);
        c *= mix(0.5, 1.0, ao);
        // Under water the mud is darker and the surface is a mirror.
        c = mix3(c, hex(0x2e241a), v.water);
        // Damp mud has a soft sheen, never a mirror; only standing water is glossy.
        let wet_rough = 0.42 + b.w * 0.12 + (white(i, 2) - 0.5) * 0.08;
        let mut rough = mix(wet_rough, 0.78, v.dry);
        rough = mix(rough, 0.6, v.pebble * 0.6);
        rough = mix(rough, 0.04, v.water);
        Texel::new(plausible(c), rough, ao)
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}

/// Fresh snow: soft wind-shaped drifts, fallen clumps, a granular crust, faint blue in the
/// hollows, and sparse ice crystals that glint without darkening. 1 m per tile.
pub fn snow(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 1.0);
    let seed = 0x5a0e_0002;
    let broad = tile.coarse(4, |p| {
        let q = tile.warp(p, 0.5, 0.1, 3, sub(seed, 7));
        Vec4::new(
            tile.fbm(p, 0.6, 4, 0.5, sub(seed, 1)),
            tile.ridged(q, 0.35, 3, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.25, 3, 0.5, sub(seed, 3)),
            0.0,
        )
    });
    let px = tile.par(|p, x, y| {
        let b = broad.at(x, y);
        let clumps = -tile.smooth_cells(p, 0.05, 1.0, 4.0, sub(seed, 4));
        let small = -tile.smooth_cells(p, 0.012, 1.0, 6.0, sub(seed, 8));
        let lumps = tile.fbm(p, 0.025, 3, 0.55, sub(seed, 5));
        // Granular crust: loose grains a few millimeters across.
        let grain = tile.value(p, 0.003, sub(seed, 6)) + tile.value(p, 0.0016, sub(seed, 9)) * 0.6;
        b.x * 0.02
            + b.y * 0.008
            + clumps * 0.005
            + small * 0.0012
            + lumps * 0.0015
            + grain * 0.00035
            + (white(y * tile.w + x, 4) - 0.5) * 0.0001
    });
    let r = relief(&tile, &px, &[0.006, 0.03, 0.1], 0.8, 0.006);
    let mut normal = r.normal;
    // Ice crystals: a few texels with tilted, mirror-smooth facets.
    let sparkle: Vec<f32> = tile.par_idx(|i| smooth(0.986, 0.99, white(i, 7)));
    let slope: Vec<Vec2> = tile.par_idx(|i| {
        if sparkle[i] > 0.0 {
            let a = white(i, 8) * std::f32::consts::TAU;
            Vec2::new(a.cos(), a.sin()) * (0.2 + 0.3 * white(i, 9))
        } else {
            Vec2::ZERO
        }
    });
    add_slope(&mut normal, &slope);
    let texels = tile.par(|_, x, y| {
        let i = y * tile.w + x;
        let b = broad.at(x, y);
        let ao = r.ao[i];
        let hollow = smooth(0.0, 0.2, r.curv[i]) + (1.0 - ao) * 2.0;
        let mut c = hex(0xf2f4f8) * (1.0 + b.z * 0.03 + (white(i, 1) - 0.5) * 0.04);
        c = mix3(c, hex(0xbfcfe6), hollow.min(1.0) * 0.7);
        let crust = smooth(0.0, 0.4, b.z);
        let rough = mix(0.75, 0.6, crust) + (white(i, 2) - 0.5) * 0.08;
        Texel::new(
            plausible(c),
            mix(rough, 0.05, sparkle[i]),
            mix(1.0, ao, 0.7),
        )
    });
    let maps = Maps::new(tile, normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default().with_subsurface(0.3))
}

/// Sand: wind ripples with gentle windward and steep lee slopes, wavy and forking crests,
/// darker heavy grains in the troughs, and speckled grains. 1 m per tile.
pub fn sand(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 1.0);
    let seed = 0x5a4d_0001;
    let broad = tile.coarse(4, |p| {
        Vec4::new(
            tile.fbm(p, 0.25, 3, 0.5, sub(seed, 1)),
            tile.fbm(p, 0.5, 3, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.35, 3, 0.5, sub(seed, 3)),
            tile.fbm(p, 0.12, 3, 0.5, sub(seed, 4)),
        )
    });
    // Ripple wave vector in whole cycles per tile (keeps the pattern tileable).
    let (ka, kb) = (2.0f32, 11.0f32);
    #[derive(Clone, Copy, Default)]
    struct S {
        h: f32,
        trough: f32,
    }
    let px = tile.par(|p, x, y| {
        let b = broad.at(x, y);
        let phase = ka * p.x + kb * p.y + b.x * 1.6 + b.w * 0.35;
        let s = phase - phase.floor();
        let profile = if s < 0.7 {
            0.5 - 0.5 * (std::f32::consts::PI * s / 0.7).cos()
        } else {
            0.5 + 0.5 * (std::f32::consts::PI * (s - 0.7) / 0.3).cos()
        };
        let amp = 0.004 * (0.55 + 0.45 * smooth(-0.4, 0.4, b.z));
        let h = profile * amp
            + b.y * 0.012
            + (tile.value(p, 0.002, sub(seed, 5)) - 0.5) * 0.00015
            + (white(y * tile.w + x, 6) - 0.5) * 0.00004;
        S {
            h,
            trough: 1.0 - profile,
        }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.003, 0.012, 0.05], 1.0, 0.003);
    let texels = tile.par(|_, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let b = broad.at(x, y);
        let mut c = hex(0xb9a686) * (1.0 + b.y * 0.12 + b.z * 0.05);
        // Heavy, darker grains collect in the troughs; crests are paler.
        c = mix3(
            c,
            c * Vec3::new(0.78, 0.76, 0.76),
            smooth(0.55, 0.95, v.trough) * 0.6,
        );
        c *= 1.0 + smooth(0.3, 0.0, v.trough) * 0.06;
        let g = white(i, 1);
        c *= if g < 0.05 {
            0.55
        } else if g > 0.985 {
            1.25
        } else {
            1.0 + (g - 0.5) * 0.12
        };
        c *= mix(0.7, 1.0, r.ao[i]);
        let rough = 0.9 + (white(i, 2) - 0.5) * 0.08;
        Texel::new(plausible(c), rough, r.ao[i])
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}

/// Packed dirt: clods, half-buried pebbles, twigs, damp patches. 1 m per tile.
pub fn dirt(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 1.0);
    let seed = 0xd147_0001;
    let broad = tile.coarse(4, |p| {
        Vec4::new(
            tile.fbm(p, 0.3, 4, 0.5, sub(seed, 1)),
            tile.fbm(p, 0.5, 3, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.1, 3, 0.5, sub(seed, 3)),
            0.0,
        )
    });
    #[derive(Clone, Copy, Default)]
    struct D {
        h: f32,
        pebble: f32,
        kind: f32,
        twig: f32,
    }
    let px = tile.par(|p, x, y| {
        let b = broad.at(x, y);
        let clods = tile.fbm(p, 0.025, 3, 0.55, sub(seed, 4));
        let mut h = b.x * 0.006 + b.z * 0.002 + clods * 0.0018;
        h += (tile.value(p, 0.0025, sub(seed, 5)) - 0.5) * 0.0004;
        // Crumbs of soil, and pebbles half sunk in it.
        let crumbs = -tile.smooth_cells(p, 0.006, 1.0, 6.0, sub(seed, 9));
        h += crumbs * 0.0008;
        // Irregular outlines (a warped lookup) and flattish, worn tops.
        let pq = p + tile.off(Vec2::new(clods, b.z) * 0.005);
        let pv = tile.voronoi(pq, 0.03, 1.0, sub(seed, 6));
        let pr = 0.12 + 0.28 * unit(mix32(pv.id ^ 4)).powi(2);
        let mut pebble = 0.0;
        if unit(pv.id) < 0.17 {
            let d = (pv.f1 / pr).min(1.0);
            let dome = smooth(1.0, 0.45, d) * (0.85 + 0.15 * (1.0 - d * d));
            let top = dome * (0.0015 + 0.004 * unit(mix32(pv.id ^ 5)));
            pebble = smooth(-0.0003, 0.0003, top - h * 0.5 - 0.0004);
            h = h.max(h * 0.5 + top);
        }
        let twig = strands(&tile, p, 0.08, 0.25, 0.06, 0.0015, sub(seed, 7));
        h += twig * 0.0012;
        D {
            h,
            pebble,
            kind: unit(mix32(pv.id ^ 6)),
            twig,
        }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.003, 0.01, 0.04], 1.8, 0.003);
    let texels = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let b = broad.at(x, y);
        let ao = r.ao[i];
        let damp = smooth(0.0, 0.4, b.y);
        let mut c = mix3(
            hex(0x6e5640),
            hex(0x5a4430),
            tile.value(p, 0.015, sub(seed, 8)),
        );
        c *= 1.0 + b.z * 0.15 + (white(i, 1) - 0.5) * 0.1;
        c = mix3(c, c * 0.65, damp);
        let pebble_c = if v.kind < 0.4 {
            hex(0x6e685e)
        } else if v.kind < 0.75 {
            hex(0x6a5a48)
        } else {
            hex(0x857660)
        } * (1.0 + (white(i, 2) - 0.5) * 0.1);
        // Dust settles on pebbles; only their tops show their own color.
        let dusty = mix3(pebble_c, c, 0.35);
        c = mix3(c, dusty, v.pebble * 0.85);
        c = mix3(c, hex(0x3e2c1e), v.twig * 0.85);
        c *= mix(0.4, 1.0, ao);
        let mut rough = 0.92 - damp * 0.15 + (white(i, 3) - 0.5) * 0.06;
        rough = mix(rough, 0.7, v.pebble);
        Texel::new(plausible(c), rough, ao)
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}

/// Clear water: a ripple normal map from gentle wind waves of many wavelengths and
/// directions, irregular swell and capillary chop, a slightly varying tint and near-mirror
/// roughness. 4 m per tile.
pub fn water(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 4.0);
    let seed = 0x3a7e_0002;
    // Waves with whole cycles per tile so the surface tiles; spread around a wind direction.
    let waves: Vec<(Vec2, f32, f32)> = (0..28)
        .map(|k| {
            let len_m = 0.05 * (16.0f32).powf(rand(seed, k));
            let a = 0.6 + (rand(seed, 100 + k) - 0.5) * 3.6;
            let cycles = (Vec2::new(a.cos(), a.sin()) * (tile.size.x / len_m)).round();
            let cycles = if cycles == Vec2::ZERO {
                Vec2::X
            } else {
                cycles
            };
            let real_len = tile.size.x / cycles.length();
            let steep = 0.008 + 0.02 * rand(seed, 200 + k);
            let amp = steep * real_len / std::f32::consts::TAU;
            (cycles, amp, rand(seed, 300 + k) * std::f32::consts::TAU)
        })
        .collect();
    let swell = tile.coarse(4, |p| {
        let q = tile.warp(p, 0.8, 0.2, 3, sub(seed, 3));
        tile.fbm(q, 0.5, 4, 0.55, sub(seed, 4))
    });
    let px = tile.par(|p, x, y| {
        let mut h = 0.0;
        for (c, a, ph) in &waves {
            h += a * (std::f32::consts::TAU * c.dot(p) + ph).sin();
        }
        h + swell.at(x, y) * 0.004 + tile.fbm(p, 0.06, 3, 0.5, sub(seed, 1)) * 0.0004
    });
    let r = relief(&tile, &px, &[0.02, 0.08], 0.2, 0.02);
    let tint = tile.coarse(8, |p| tile.fbm(p, 1.0, 3, 0.5, sub(seed, 2)));
    let texels = tile.par(|_, x, y| {
        let i = y * tile.w + x;
        let t = tint.at(x, y);
        let c = mix3(hex(0x1d4a55), hex(0x214e4c), smooth(-0.3, 0.3, t));
        let crest = smooth(0.0, -0.1, r.curv[i]);
        Texel::new(c, 0.03 + crest * 0.02 + (white(i, 1) - 0.5) * 0.01, 1.0)
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    let mut base = StandardMaterial::default().with_alpha(0.88);
    base.reflectance = 0.35;
    base.receives_wetness = false;
    Generated::new(maps, base)
}
