//! Plants: a dense layer of leaves, and moss cushions.

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::StandardMaterial;

use super::Generated;
use super::noise::{mix32, rand, sub, unit};
use super::textures::{Maps, Texel, Tile, hex, mix, mix3, plausible, relief, smooth, white};

/// One leaf of the layer, in meters within the tile.
#[derive(Clone, Copy)]
struct Leaf {
    base: Vec2,
    /// Unit direction from base to tip.
    dir: Vec2,
    len: f32,
    half_width: f32,
    /// Layer height (m): higher leaves cover lower ones.
    z: f32,
    tone: f32,
    kind: f32,
}

/// A dense, layered mass of leaves: pointed oval leaves at every angle, each with a midrib,
/// side veins, a domed cross-section and its own tint (a few yellowing or dry), with dark
/// gaps between them. 0.5 m per tile.
pub fn leaves(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.5);
    let seed = 0x1eaf_0001;
    let per = tile.period(0.03);
    let cell = tile.size / Vec2::new(per[0] as f32, per[1] as f32);
    const PER_CELL: usize = 3;
    // Leaves stored per grid cell; each lies within a cell of its base point.
    let mut cells: Vec<[Leaf; PER_CELL]> = Vec::with_capacity((per[0] * per[1]) as usize);
    for cy in 0..per[1] {
        for cx in 0..per[0] {
            let cs = sub(seed, (cy * per[0] + cx) as u32);
            cells.push(std::array::from_fn(|k| {
                let r = |j: u32| rand(cs, k as u32 * 16 + j);
                let len = 0.032 + 0.028 * r(0);
                let a = r(1) * std::f32::consts::TAU;
                let dir = Vec2::new(a.cos(), a.sin());
                let center = (Vec2::new(cx as f32, cy as f32) + Vec2::new(r(2), r(3))) * cell;
                Leaf {
                    base: center - dir * len * 0.5,
                    dir,
                    len,
                    half_width: len * (0.19 + 0.08 * r(4)),
                    z: r(5) * 0.02,
                    tone: r(6),
                    kind: r(7),
                }
            }));
        }
    }
    #[derive(Clone, Copy, Default)]
    struct L {
        h: f32,
        leaf: Option<(u16, u8)>,
        vein: f32,
        edge: f32,
        t: f32,
    }
    let px = tile.par(|p, _, _| {
        let m = p * tile.size;
        let cx = ((p.x * per[0] as f32) as i32).min(per[0] - 1);
        let cy = ((p.y * per[1] as f32) as i32).min(per[1] - 1);
        let mut best = L {
            h: -0.03,
            ..Default::default()
        };
        for j in -1..=1 {
            for i in -1..=1 {
                let ix = (cx + i).rem_euclid(per[0]);
                let iy = (cy + j).rem_euclid(per[1]);
                // Shift so a leaf in a wrapped neighbor is measured from the right side.
                let shift = Vec2::new(
                    ((cx + i) - ix) as f32 * cell.x,
                    ((cy + j) - iy) as f32 * cell.y,
                );
                let idx = (iy * per[0] + ix) as usize;
                for (k, leaf) in cells[idx].iter().enumerate() {
                    let d = m - (leaf.base + shift);
                    let along = d.dot(leaf.dir);
                    if along <= 0.0 || along >= leaf.len {
                        continue;
                    }
                    let t = along / leaf.len;
                    let across = d.x * leaf.dir.y - d.y * leaf.dir.x;
                    // Pointed oval outline, widest a little below the middle.
                    let profile = (std::f32::consts::PI * t.powf(0.8)).sin().powf(0.9);
                    let hw = leaf.half_width * profile;
                    if across.abs() >= hw {
                        continue;
                    }
                    let s = across / hw;
                    // Domed across the blade, rising toward the tip, midrib pressed in.
                    let h = leaf.z + (1.0 - s * s) * 0.0018 + t * 0.003
                        - smooth(0.08, 0.0, s.abs()) * 0.00025;
                    if h > best.h {
                        // Side veins run from the midrib toward the tip at about 45°.
                        let vcoord = (along - across.abs() * 0.9) / (leaf.len * 0.11);
                        let side = smooth(0.12, 0.0, (vcoord - vcoord.round()).abs())
                            * smooth(0.95, 0.6, s.abs());
                        let mid = smooth(0.06, 0.0, s.abs());
                        best = L {
                            h,
                            leaf: Some((idx as u16, k as u8)),
                            vein: mid.max(side * 0.6),
                            edge: smooth(0.75, 1.0, s.abs()),
                            t,
                        };
                    }
                }
            }
        }
        best.h += best.vein * 0.0001;
        best
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.002, 0.008, 0.03], 2.0, 0.002);
    let texels = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let ao = r.ao[i];
        let Some((ci, k)) = v.leaf else {
            // Deep gaps: dark litter in shadow (the darkness is mostly occlusion).
            return Texel::new(hex(0x2c3c22), 0.9, ao * 0.5);
        };
        let leaf = cells[ci as usize][k as usize];
        let mut c = mix3(hex(0x3e5e22), hex(0x5a7a2c), leaf.tone);
        if leaf.kind < 0.03 {
            c = mix3(hex(0x7a6a3a), hex(0x6a5a32), leaf.tone);
        } else if leaf.kind < 0.12 {
            c = mix3(c, hex(0x8a9a3a), 0.45);
        } else if leaf.kind > 0.8 {
            c = mix3(c, hex(0x2c4a1e), 0.6);
        }
        // Darker toward the stalk, lighter toward the tip.
        c *= mix(0.8, 1.08, v.t);
        // Veins paler; a little browning at the margins and tips.
        c = mix3(c, c * Vec3::new(1.35, 1.3, 1.1), v.vein * 0.6);
        let brown = smooth(0.7, 1.0, v.edge.max(v.t)) * smooth(0.5, 0.9, leaf.tone) * 0.35;
        c = mix3(c, hex(0x6a5a2a), brown);
        c *= 1.0 + (tile.value(p, 0.004, sub(seed, 9)) - 0.5) * 0.1 + (white(i, 1) - 0.5) * 0.04;
        c *= mix(0.25, 1.0, ao);
        let rough = 0.48 + v.vein * 0.1 + leaf.kind * 0.1 + brown * 0.2;
        Texel::new(plausible(c), rough, ao)
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default().with_subsurface(0.4))
}

/// Moss: soft, merging cushions of tiny upright shoots, bright yellow-green tips over dark
/// depths, hue shifts from clump to clump, and brown dead patches. 0.5 m per tile.
pub fn moss(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.5);
    let seed = 0x4055_0002;
    let broad = tile.coarse(4, |p| {
        let q = tile.warp(p, 0.1, 0.012, 3, sub(seed, 9));
        Vec4::new(
            tile.fbm(p, 0.15, 3, 0.5, sub(seed, 1)),
            tile.fbm(p, 0.1, 3, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.2, 3, 0.5, sub(seed, 3)),
            -tile.smooth_cells(q, 0.045, 1.0, 5.0, sub(seed, 4)),
        )
    });
    #[derive(Clone, Copy, Default)]
    struct Mo {
        h: f32,
        tip: f32,
        lump: f32,
    }
    let px = tile.par(|p, x, y| {
        let b = broad.at(x, y);
        let lump = b.w + tile.fbm(p, 0.02, 2, 0.5, sub(seed, 5)) * 0.12;
        // Shoots: each a tiny star of leaves seen from above, leaning every way.
        let shoot = tile.voronoi(p, 0.0024, 1.0, sub(seed, 6));
        let tip = smooth(0.6, 0.0, shoot.f1) * (0.6 + 0.4 * unit(mix32(shoot.id)));
        let fuzz = tile.value(p, 0.0011, sub(seed, 7)) + (white(y * tile.w + x, 5) - 0.5) * 0.6;
        let h = lump * 0.012 + b.x * 0.004 + tip * 0.0009 + fuzz * 0.0004;
        Mo { h, tip, lump }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.0015, 0.006, 0.025], 2.0, 0.002);
    let texels = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let b = broad.at(x, y);
        let ao = r.ao[i];
        let high = smooth(-0.6, -0.15, v.lump);
        let hue = smooth(0.2, 0.8, tile.value(p, 0.06, sub(seed, 8)) + b.z * 0.5);
        let deep = mix3(hex(0x3a4220), hex(0x4a3e26), smooth(0.2, 0.6, b.y));
        let mid = mix3(hex(0x3e5e26), hex(0x6a7e24), hue);
        let tips = mix3(hex(0x86a23e), hex(0xb0b048), hue);
        let mut c = mix3(deep, mid, smooth(0.0, 0.7, high * mix(0.4, 1.0, ao)));
        c = mix3(c, tips, v.tip * high * 0.65);
        let dead = smooth(0.3, 0.6, b.y) * smooth(0.0, 0.3, b.z + 0.2);
        c = mix3(c, mix3(hex(0x5e4e2c), hex(0x8a7a48), v.tip), dead * 0.75);
        c *= 1.0 + (white(i, 1) - 0.5) * 0.16;
        c *= mix(0.6, 1.0, ao);
        let rough = 0.9 + (white(i, 2) - 0.5) * 0.08 - v.tip * 0.05;
        Texel::new(plausible(c), rough, ao)
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default().with_subsurface(0.3))
}
