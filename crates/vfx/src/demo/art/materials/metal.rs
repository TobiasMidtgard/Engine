//! Metals: forged iron with rust, and aged bronze with patina.

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::StandardMaterial;

use super::Generated;
use super::noise::sub;
use super::textures::{Maps, Texel, Tile, hex, mix, mix3, plausible, relief, smooth, white};

/// Forged iron: overlapping soft hammer dishes over a slightly wavy bar, a dark mottled oxide
/// skin rubbed back to bare metal on the high spots, pitting, and rust that spreads from
/// hollows and blooms with freckled fringes. 0.5 m per tile.
pub fn iron(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.5);
    let seed = 0x1404_0002;
    let broad = tile.coarse(4, |p| {
        let q = tile.warp(p, 0.2, 0.03, 3, sub(seed, 11));
        Vec4::new(
            tile.fbm(p, 0.15, 4, 0.5, sub(seed, 1)),
            tile.fbm(q, 0.12, 5, 0.55, sub(seed, 2)),
            tile.fbm(p, 0.2, 3, 0.5, sub(seed, 3)),
            tile.fbm(q, 0.05, 3, 0.5, sub(seed, 4)),
        )
    });
    #[derive(Clone, Copy, Default)]
    struct I {
        h: f32,
        rust: f32,
        pit: f32,
        freckle: f32,
        base: f32,
    }
    let px = tile.par(|p, x, y| {
        let b = broad.at(x, y);
        // Hammer blows: soft overlapping dishes at two sizes.
        let q = p + tile.off(Vec2::new(b.w, b.x) * 0.006);
        let big = tile.smooth_cells(q, 0.03, 0.9, 5.0, sub(seed, 5));
        let small = tile.smooth_cells(q, 0.013, 0.9, 5.0, sub(seed, 6));
        let mut h = big * 0.002 + small * 0.0005 + b.x * 0.0008;
        // Corrosion pits and fine grain.
        let pit = smooth(0.76, 0.92, tile.value(p, 0.002, sub(seed, 7)));
        h += (tile.value(p, 0.0012, sub(seed, 8)) - 0.5) * 0.00003;
        // Rust: grows from a few broad damp regions; freckles scatter around its edges.
        let fringe = tile.value(p, 0.003, sub(seed, 9));
        let base = b.z * 0.9 + b.y * 0.5 + b.w * 0.2 - 0.05;
        let rust = smooth(0.05, 0.32, base + (fringe - 0.5) * 0.12);
        let freckle = smooth(-0.1, 0.08, base) * smooth(0.82, 0.9, fringe) * (1.0 - rust);
        let crust = tile.value(p, 0.0025, sub(seed, 10));
        h += (rust + freckle) * (0.00006 + crust * 0.00012) - pit * 0.0002 * (1.0 - rust);
        I {
            h,
            rust,
            pit,
            freckle,
            base,
        }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.002, 0.008, 0.03], 1.2, 0.008);
    let texels = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let b = broad.at(x, y);
        let ao = r.ao[i];
        // High spots rubbed back to bright metal; the rest keeps a dark, mottled oxide skin.
        let wear = smooth(-0.004, -0.04, r.curv[i]) * smooth(-0.3, 0.2, -b.z) * (1.0 - v.rust);
        let mottle = tile.fbm(p, 0.03, 2, 0.5, sub(seed, 12));
        // Mill scale and oxide: a dark blue-gray skin, mottled, slightly browner in patches.
        let oxide = hex(0x666462) * (1.0 + mottle * 0.25 + b.x * 0.12)
            + Vec3::new(0.015, 0.008, 0.0) * smooth(0.0, 0.5, b.w);
        let bare = hex(0xa8a6a2) * (1.0 + (white(i, 1) - 0.5) * 0.05);
        let metal_c = mix3(oxide, bare, wear);
        let metal_rough = mix(0.62 + mottle * 0.12, 0.36, wear);
        let metalness = mix(0.85, 1.0, wear);
        // Rust: dark brown crust, a warmer brown where fresh, thin orange only at the edges.
        let tone = tile.value(p, 0.006, sub(seed, 13));
        let rust = (v.rust + v.freckle * 0.8).min(1.0);
        let fresh = smooth(0.2, 0.9, tone) * smooth(1.0, 0.3, v.rust);
        let rust_c = mix3(hex(0x3a2418), hex(0x6a3a20), fresh)
            * (1.0 + (tile.value(p, 0.0015, sub(seed, 14)) - 0.5) * 0.2);
        // A thin brown stain bleeds out around each bloom.
        let stain = smooth(-0.25, 0.05, v.base) * (1.0 - rust);
        let metal_c = mix3(metal_c, hex(0x4e3a2c), stain * 0.6);
        let albedo = mix3(metal_c, rust_c, rust) * mix(0.6, 1.0, ao);
        let rough =
            mix(metal_rough + stain * 0.15, 0.9, rust) + v.pit * 0.08 + (white(i, 3) - 0.5) * 0.03;
        Texel {
            albedo: albedo.clamp(Vec3::splat(0.02), Vec3::splat(0.92)),
            rough,
            metal: mix(metalness * (1.0 - stain * 0.4), 0.0, rust),
            ao,
            emissive: Vec3::ZERO,
            alpha: 1.0,
        }
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}

/// Aged cast bronze: dark brown oxidized metal, polished gold where handled, and a chalky
/// green verdigris crust that fills the hollows and creeps out in crumbly patches. 0.5 m
/// per tile.
pub fn bronze(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.5);
    let seed = 0xb404_0002;
    let broad = tile.coarse(4, |p| {
        let q = tile.warp(p, 0.15, 0.02, 3, sub(seed, 11));
        Vec4::new(
            tile.fbm(p, 0.08, 4, 0.5, sub(seed, 1)),
            tile.fbm(q, 0.1, 5, 0.55, sub(seed, 2)),
            tile.fbm(p, 0.3, 3, 0.5, sub(seed, 3)),
            tile.fbm(q, 0.04, 3, 0.5, sub(seed, 4)),
        )
    });
    #[derive(Clone, Copy, Default)]
    struct B {
        h: f32,
        pore: f32,
        crumb: f32,
    }
    let px = tile.par(|p, x, y| {
        let b = broad.at(x, y);
        let dents = tile.smooth_cells(p, 0.025, 0.9, 8.0, sub(seed, 5));
        let pore = smooth(0.8, 0.93, tile.value(p, 0.0025, sub(seed, 6)));
        let crumb = tile.fbm(p, 0.012, 2, 0.5, sub(seed, 7)) + 0.5;
        let h = b.x * 0.0007 + dents * 0.0003 - pore * 0.00025
            + (tile.value(p, 0.0012, sub(seed, 8)) - 0.5) * 0.00002;
        B { h, pore, crumb }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.002, 0.008, 0.03], 1.2, 0.008);
    // Corrosion grows where the metal is sheltered (dents and seams, not single pores) and on
    // damp blotches: a thin olive-brown tarnish first, then a raised green crust.
    let corrosion: Vec<(f32, f32)> = tile.par(|_, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let b = broad.at(x, y);
        let shelter =
            smooth(0.0, 0.03, r.curv[i]) * (1.0 - v.pore) * 0.4 + smooth(0.95, 0.7, r.ao[i]);
        let amount = shelter * 0.2 + b.y * 0.5 + b.z * 0.6 + b.w * 0.2 + (v.crumb - 0.5) * 0.12;
        // The crust breaks up into crumbs toward its edges.
        let pat = smooth(0.06, 0.3, amount);
        (
            smooth(-0.2, 0.08, amount),
            pat * smooth(0.25, 0.55, v.crumb * 0.6 + pat * 0.6),
        )
    });
    let crust: Vec<f32> = tile.par_idx(|i| height[i] + corrosion[i].1 * 0.00015);
    let r2 = relief(&tile, &crust, &[0.002, 0.008], 1.2, 0.002);
    let texels = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let b = broad.at(x, y);
        let (tarnish, pat) = corrosion[i];
        let ao = r.ao[i].min(r2.ao[i]);
        let polish = smooth(-0.005, -0.04, r.curv[i]) * (1.0 - tarnish) * smooth(-0.3, 0.3, b.z);
        // Oxidized bronze: a muted dark brown, dulled to olive where tarnished.
        let aged = mix3(hex(0x5e4a36), hex(0x6e553a), smooth(-0.3, 0.3, b.z + b.x));
        let aged = mix3(aged, hex(0x55513a), tarnish * 0.8);
        let metal_c = mix3(aged, hex(0xb08454), polish) * (1.0 + (white(i, 1) - 0.5) * 0.03);
        // Verdigris: olive where thin, muted blue-green, chalky and pale where thick.
        let tone = tile.value(p, 0.01, sub(seed, 9));
        let mut green = mix3(
            hex(0x3f6a52),
            hex(0x6a8e74),
            smooth(0.35, 0.9, tone + (v.crumb - 0.5) * 0.4),
        );
        green = mix3(hex(0x3e5038), green, smooth(0.1, 0.9, pat));
        // Casting pores hold dark grime.
        let albedo = mix3(metal_c, green, pat) * mix(0.6, 1.0, ao) * (1.0 - v.pore * 0.3);
        let rough =
            mix(mix(0.45 + tarnish * 0.2, 0.3, polish), 0.85, pat) + (white(i, 2) - 0.5) * 0.03;
        let metal = (1.0 - pat) * (1.0 - tarnish * 0.35);
        Texel {
            albedo: if metal < 0.5 {
                plausible(albedo)
            } else {
                albedo.clamp(Vec3::splat(0.02), Vec3::splat(0.92))
            },
            rough,
            metal,
            ao,
            emissive: Vec3::ZERO,
            alpha: 1.0,
        }
    });
    let maps = Maps::new(tile, r2.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}
