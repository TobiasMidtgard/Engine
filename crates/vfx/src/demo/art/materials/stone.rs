//! Stone surfaces: court flagstones, dressed granite, basalt, natural rock, rune inlays and
//! crystal.

use engine_core::math::{Vec2, Vec3, Vec4};
use engine_render::StandardMaterial;

use super::Generated;
use super::noise::{floor, hash2, mix32, rand, sub, unit};
use super::textures::{
    Coarse, Maps, Texel, Tile, hex, mix, mix3, plausible, relief, smooth, white,
};

/// One course (row) of paving slabs, in meters.
struct Course {
    y0: f32,
    h: f32,
    offset: f32,
    /// Slab start positions along the course, ascending, the first at 0.
    cuts: Vec<f32>,
    /// Index of the course's first slab among all slabs.
    first: usize,
}

/// Rows of slabs filling a `size` tile exactly (so the paving tiles): `rows` courses with
/// heights in `course_h` and slab lengths in `slab_w` (meters, before normalizing).
fn courses(
    size: Vec2,
    seed: u32,
    rows: usize,
    course_h: (f32, f32),
    slab_w: (f32, f32),
) -> Vec<Course> {
    let hs: Vec<f32> = (0..rows)
        .map(|r| mix(course_h.0, course_h.1, rand(seed, r as u32)))
        .collect();
    let total: f32 = hs.iter().sum();
    let mut y = 0.0;
    let mut out = Vec::new();
    let mut first = 0;
    for (r, h) in hs.iter().enumerate() {
        let h = h / total * size.y;
        let cs = sub(seed, 100 + r as u32);
        let mut widths = Vec::new();
        let mut sum = 0.0;
        let mut k = 0;
        while sum < size.x - slab_w.0 * 0.5 {
            let w = mix(slab_w.0, slab_w.1, rand(cs, k));
            widths.push(w);
            sum += w;
            k += 1;
        }
        let mut cuts = Vec::with_capacity(widths.len());
        let mut x = 0.0;
        for w in &widths {
            cuts.push(x);
            x += w / sum * size.x;
        }
        let n = cuts.len();
        out.push(Course {
            y0: y,
            h,
            offset: rand(cs, 999) * size.x,
            cuts,
            first,
        });
        first += n;
        y += h;
    }
    out
}

/// Random parameters per slab (`count` slabs, `K` values each in 0..1), so per-texel code
/// reads a table instead of hashing.
fn slab_params<const K: usize>(count: usize, seed: u32) -> Vec<[f32; K]> {
    (0..count)
        .map(|i| {
            let id = sub(seed, 7000 + i as u32);
            std::array::from_fn(|k| rand(id, k as u32))
        })
        .collect()
}

/// The slab under `m` (meters): (slab index, position inside the slab from its corner, slab
/// size).
fn locate(courses: &[Course], m: Vec2, size: Vec2) -> (usize, Vec2, Vec2) {
    let y = m.y.rem_euclid(size.y);
    let c = courses
        .iter()
        .find(|c| y < c.y0 + c.h)
        .unwrap_or(&courses[courses.len() - 1]);
    let x = (m.x - c.offset).rem_euclid(size.x);
    let j = c.cuts.partition_point(|&s| s <= x).saturating_sub(1);
    let x0 = c.cuts[j];
    let x1 = c.cuts.get(j + 1).copied().unwrap_or(size.x);
    (
        c.first + j,
        Vec2::new(x - x0, y - c.y0),
        Vec2::new(x1 - x0, c.h),
    )
}

/// Signed distance inside a rounded rectangle of `dims` (positive inside), from its corner.
fn inside_rounded(local: Vec2, dims: Vec2, inset: f32, corner: f32) -> f32 {
    let half = dims * 0.5 - Vec2::splat(inset);
    let c = (local - dims * 0.5).abs();
    let q = c - half + Vec2::splat(corner);
    let outside = q.max(Vec2::ZERO).length() + q.x.max(q.y).min(0.0) - corner;
    -outside
}

#[derive(Clone, Copy, Default)]
struct FlagPx {
    h: f32,
    /// Distance into the slab from its worn edge (negative in the joint).
    d_in: f32,
    slab: usize,
    chip: f32,
    crack: f32,
    lichen: f32,
    moss: f32,
    pebble: f32,
    /// 1 in the joint between slabs.
    joint: f32,
}

/// Worn sandstone and limestone court paving in courses, with rounded worn edges, chips,
/// cracks, pits, lichen, stains and dirt joints. 2 m per tile.
pub fn flagstone(shrink: u32) -> Generated {
    let tile = Tile::new(1024 >> shrink, 1024 >> shrink, 2.0);
    let size = tile.size;
    let seed = 0xf1a6_0001;
    let rows = courses(size, seed, 4, (0.4, 0.62), (0.42, 0.9));
    let count = rows.last().map_or(0, |c| c.first + c.cuts.len());
    let params = slab_params::<48>(count, seed);

    // Low-frequency layers on coarse grids.
    let shape = tile.coarse(4, |p| {
        let wv = tile.warp(p, 0.5, 0.008, 3, sub(seed, 1)) - p;
        Vec4::new(
            wv.x,
            wv.y,
            tile.fbm(p, 0.1, 3, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.35, 4, 0.5, sub(seed, 4)),
        )
    });
    let broad = tile.coarse(4, |p| {
        Vec4::new(
            tile.fbm(p, 1.0, 2, 0.5, sub(seed, 5)),
            tile.fbm(p, 0.25, 4, 0.55, sub(seed, 22)),
            tile.fbm(p, 0.6, 3, 0.5, sub(seed, 23)),
            tile.fbm(p, 0.8, 4, 0.55, sub(seed, 25)),
        )
    });
    let mid = tile.coarse(2, |p| {
        let lv = tile.voronoi(p, 0.06, 1.0, sub(seed, 8));
        let lr = 0.12 + 0.3 * unit(mix32(lv.id ^ 5));
        // Lichen colonies: signed distance to the colony rim (cell units), -1 when none.
        let colony = if unit(lv.id) < 0.28 { lr - lv.f1 } else { -1.0 };
        Vec4::new(
            tile.fbm(p, 0.12, 4, 0.55, sub(seed, 14)),
            tile.fbm(p, 0.15, 4, 0.6, sub(seed, 11)),
            tile.fbm(p, 0.3, 3, 0.6, sub(seed, 26)),
            colony,
        )
    });

    let px: Vec<FlagPx> = tile.par(|p, x, y| {
        let sh = shape.at(x, y);
        let md = mid.at(x, y);
        let q = p + Vec2::new(sh.x, sh.y);
        let (slab, local, dims) = locate(&rows, q * size, size);
        let sp = &params[slab];
        let s = |k: usize| sp[k];
        let joint = 0.005 + 0.004 * s(1);
        let corner = 0.01 + 0.04 * s(2);
        let fine = tile.fbm(p, 0.02, 3, 0.5, sub(seed, 6));
        let mut d_in = inside_rounded(local, dims, joint, corner) + sh.z * 0.004 + fine * 0.0015;

        // Chips taken out of the edges: scalloped bites whose reach follows noise.
        let mut chip = 0.0;
        let mut chip_h = 0.0;
        if d_in < 0.045 {
            let n = tile.fbm(p, 0.05, 3, 0.55, sub(seed, 3));
            let reach = (n - 0.05).max(0.0) * 0.09;
            if reach > 0.0 && d_in < reach + 0.002 {
                let wall = smooth(reach + 0.0015, reach - 0.0015, d_in);
                let scoop = (1.0 - (d_in / reach).clamp(0.0, 1.0)).powf(0.7);
                chip = wall;
                chip_h = wall * (0.0015 + 0.004 * scoop) * smooth(0.0, 0.01, reach);
            }
        }

        // Slab body: level and tilt per slab, worn rounding toward the edges.
        let tilt = Vec2::new(s(3) - 0.5, s(4) - 0.5) * 0.006;
        let mut h = (s(5) - 0.5) * 0.005 + tilt.dot(local - dims * 0.5);
        let wear_r = 0.015 + 0.035 * s(6);
        let wear_depth = 0.003 + 0.006 * s(7);
        let e = (1.0 - (d_in / wear_r).clamp(0.0, 1.0)).powi(2);
        h -= wear_depth * e;
        // Surface relief: broad undulation, dished traffic wear, fine sandy grain.
        h += sh.w * 0.002 + fine * 0.0004;
        h -= smooth(-0.2, 0.6, broad.at(x, y).x) * 0.0015;
        // Bedding layers show as faint ridges along the slab on some stones.
        let bed = (local.y * (50.0 + 90.0 * s(8)) + md.x * 5.0).sin();
        h += bed * 0.00025 * smooth(0.45, 0.9, s(9));
        // Pits and sandy grain.
        let pit = smooth(0.72, 0.9, tile.value(p, 0.006, sub(seed, 7)));
        h -= pit * 0.0007;
        h += (tile.value(p, 0.0016, sub(seed, 15)) - 0.5) * 0.00012;
        // Chips: conchoidal scoops with crisp walls.
        h -= chip_h;

        // Cracks across some slabs.
        let mut crack = 0.0;
        if s(10) < 0.3 {
            let a = s(11) * std::f32::consts::PI;
            let n = Vec2::new(a.cos(), a.sin());
            let c = dims * Vec2::new(0.3 + 0.4 * s(12), 0.3 + 0.4 * s(13));
            let dist = ((local - c).dot(n) + md.x * 0.04 + fine * 0.002).abs();
            let along = (local - c).dot(Vec2::new(-n.y, n.x));
            let reach = dims.x.max(dims.y) * (0.25 + 0.5 * s(15));
            let fade = smooth(reach, reach * 0.5, along.abs());
            crack = (1.0 - smooth(0.0003, 0.0016, dist)) * fade;
            h -= crack * 0.004;
        }

        // Lichen crusts: small colonies, more toward the slab edges.
        let edge_bias = smooth(0.2, 0.0, d_in);
        let mut lichen = 0.0;
        if md.w > -0.5 && s(17) < 0.25 + 0.75 * edge_bias {
            let rim = md.w + fine * 0.08;
            lichen = smooth(0.0, 0.05, rim);
            h += lichen * 0.0003;
        }

        // The joint: packed dirt with grit and moss, well below the slabs.
        let stone_h = if d_in > 0.0 { h } else { h + d_in * 3.0 };
        let mut jdata = (0.0, 0.0, -1.0f32);
        if d_in < 0.004 {
            let grit = tile.voronoi(p, 0.006, 1.0, sub(seed, 10));
            let pebble = smooth(0.45, 0.2, grit.f1) * (unit(grit.id) < 0.5) as u8 as f32;
            let moss = smooth(0.0, 0.25, md.y);
            let joint_h = -0.012
                + fine * 0.0015
                + pebble * 0.0015
                + moss * (0.003 + tile.noise(p, 0.004, sub(seed, 13)) * 0.0012);
            jdata = (pebble, moss, joint_h);
        }
        let joint_h = jdata.2;
        let joint = smooth(-0.0004, 0.0004, joint_h - stone_h);
        d_in = d_in.min(0.03);
        FlagPx {
            h: stone_h.max(joint_h),
            d_in,
            slab,
            chip: chip * (1.0 - joint),
            crack,
            lichen: lichen * (1.0 - joint),
            moss: jdata.1 * joint,
            pebble: jdata.0 * joint,
            joint,
        }
    });

    let height: Vec<f32> = px.iter().map(|p| p.h).collect();
    let r = relief(&tile, &height, &[0.005, 0.014, 0.04, 0.12], 2.0, 0.005);

    // Stone tones (sRGB): buff sandstone, gray limestone, weathered brown, pinkish, greenish.
    let tones = [
        hex(0xb5a487),
        hex(0xa39e93),
        hex(0x8c7f6c),
        hex(0xab9482),
        hex(0x9a9884),
    ];
    let mottle = tile.coarse(2, |p| tile.fbm(p, 0.07, 2, 0.5, sub(seed, 28)));
    let shaded = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let f = px[i];
        let sp = &params[f.slab];
        let s = |k: usize| sp[k];
        let ao = r.ao[i];
        let curv = r.curv[i];
        let br = broad.at(x, y);
        let md = mid.at(x, y);
        let white = |k: u32| unit(mix32(i as u32 ^ k.wrapping_mul(0x9e37_79b9)));

        // Joint material.
        let mut jc = hex(0x4e4234) * mix(0.8, 1.15, white(1));
        jc = mix3(jc, hex(0x8a8172) * mix(0.8, 1.1, white(2)), f.pebble * 0.8);
        jc = mix3(
            jc,
            mix3(hex(0x4c5a26), hex(0x6f7532), white(3) * 0.5 + md.y),
            f.moss * 0.9,
        );
        let jc = jc * mix(0.3, 1.0, ao);
        let j_rough = mix(0.96, 0.85, f.moss * 0.3);
        if f.joint >= 1.0 {
            return (plausible(jc), j_rough, ao);
        }
        // Base tone per slab (a blend of two tones, so no two slabs match).
        let a = (s(30) * 5.0) as usize % 5;
        let b = (a + 1 + (s(31) * 4.0) as usize) % 5;
        let mut c = mix3(tones[a], tones[b], s(32) * 0.5) * mix(0.78, 1.1, s(33));
        // Mottling, warm/cool drift and faint bedding bands.
        c *= 1.0 + br.y * 0.25 + mottle.at(x, y) * 0.12;
        c = mix3(c, c * Vec3::new(1.07, 0.98, 0.88), smooth(-0.15, 0.4, br.z));
        // Whole cycles per tile so the bands tile.
        let cycles = (1.0 + 6.0 * s(35)).round();
        let band = (md.x * 3.0 + p.y * std::f32::consts::TAU * cycles).sin();
        c *= 1.0 + band * 0.03 * s(36);
        // Fine mineral speckle.
        let speck = tile.value(p, 0.003, sub(seed, 24));
        c *= 1.0 - smooth(0.78, 0.92, speck) * 0.3 + (white(4) - 0.5) * 0.06;
        // Stains: dark water and grime marks, heavier near joints.
        let near_joint = smooth(0.05, 0.0, f.d_in);
        let stain = (smooth(0.05, 0.45, br.w) * 0.5 + near_joint * 0.4).min(0.75);
        c = mix3(c, c * Vec3::new(0.6, 0.58, 0.54), stain);
        // Rust-brown streaks on a few slabs.
        let iron = smooth(0.2, 0.55, md.z) * smooth(0.65, 0.9, s(34));
        c = mix3(c, c * Vec3::new(1.08, 0.84, 0.62), iron * 0.6);
        // Fresh stone in chips is lighter; cracks and cavities collect grime.
        c = mix3(c, c * Vec3::new(1.12, 1.1, 1.05), f.chip * 0.8);
        c *= 1.0 - f.crack * 0.55;
        // Worn edges lighter, cavities darker.
        let convex = smooth(0.0, -0.08, curv);
        let concave = smooth(0.0, 0.12, curv);
        c *= 1.0 + convex * 0.1 - concave * 0.25;
        c *= mix(0.5, 1.0, ao);
        // Lichen: crusty gray-green and ochre colonies, a few dark ones.
        if f.lichen > 0.0 {
            let kind = s(40 + (md.w * 37.0).abs() as usize % 7);
            let base = if kind < 0.5 {
                hex(0x9c9f86)
            } else if kind < 0.85 {
                hex(0xa89a62)
            } else {
                hex(0x4a4a40)
            };
            let dots = smooth(0.4, 0.7, tile.value(p, 0.002, sub(seed, 27)));
            let lc = base * mix(0.75, 1.05, dots) * mix(0.6, 1.0, ao);
            c = mix3(c, lc, f.lichen * 0.75);
        }

        // Roughness: traffic polishes the high, open areas; grime and lichen are rough.
        let polish = smooth(-0.002, 0.003, f.h) * smooth(-0.3, 0.3, br.y) * 0.14;
        let mut rough = 0.84 - polish - convex * 0.05;
        rough += stain * 0.05 + f.lichen * 0.1 + f.chip * 0.04 + concave * 0.06;
        rough += (white(5) - 0.5) * 0.06;
        let c = mix3(c, jc, f.joint);
        let rough = mix(rough, j_rough, f.joint);
        (plausible(c), rough.clamp(0.4, 1.0), ao)
    });

    let maps = Maps {
        tile,
        albedo: shaded.iter().map(|s| s.0).collect(),
        alpha: None,
        normal: r.normal,
        ao: shaded.iter().map(|s| s.2).collect(),
        rough: shaded.iter().map(|s| s.1).collect(),
        metal: vec![0.0; tile.texels()],
        emissive: None,
    };
    Generated::new(maps, StandardMaterial::default())
}

/// Light dressed granite: interlocking feldspar, quartz and biotite grains with a fine
/// point-dressed surface, glossier quartz and mica, and soft weathering. 0.8 m per tile.
pub fn carved_stone(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.8);
    let seed = 0xca4e_0001;
    let broad = tile.coarse(4, |p| {
        Vec4::new(
            tile.fbm(p, 0.4, 3, 0.5, sub(seed, 1)),
            tile.fbm(p, 0.12, 4, 0.55, sub(seed, 2)),
            tile.fbm(p, 0.25, 4, 0.5, sub(seed, 3)),
            tile.fbm(p, 0.05, 3, 0.5, sub(seed, 4)),
        )
    });
    // Feldspar, quartz, biotite, pink feldspar.
    let colors = [hex(0xcfcac0), hex(0x9e9b97), hex(0x2e2b28), hex(0xc9b5a8)];
    let rough = [0.6, 0.4, 0.34, 0.6];
    let lift = [0.0, 0.00006, -0.00012, 0.0];
    #[derive(Clone, Copy, Default)]
    struct G {
        h: f32,
        mineral: usize,
        tint: f32,
        boundary: f32,
        pick: f32,
    }
    let px = tile.par(|p, x, y| {
        let b = broad.at(x, y);
        // Dark minerals cluster; feldspar sometimes grows into big crystals.
        let cluster = smooth(-0.25, 0.35, b.w);
        let g = tile.voronoi(p + Vec2::splat(b.w * 0.004), 0.006, 1.0, sub(seed, 5));
        let k = unit(g.id);
        let mut mineral = if k < 0.55 {
            0
        } else if k < 0.84 {
            1
        } else if k < 0.84 + 0.1 * cluster {
            2
        } else {
            3
        };
        let mut tint = unit(mix32(g.id ^ 0x33));
        let big = tile.voronoi(p, 0.02, 0.8, sub(seed, 14));
        if unit(big.id) < 0.22 {
            let a = unit(mix32(big.id)) * std::f32::consts::PI;
            let (sn, cs) = a.sin_cos();
            let t = big.to_point;
            let q = Vec2::new(t.x * cs + t.y * sn, -t.x * sn + t.y * cs);
            if q.x.abs() < 0.32 && q.y.abs() < 0.2 {
                mineral = if unit(mix32(big.id ^ 1)) < 0.7 { 0 } else { 3 };
                tint = unit(mix32(big.id ^ 2));
            }
        }
        // Small biotite flakes scattered through the other grains.
        let fl = tile.voronoi(p, 0.0025, 1.0, sub(seed, 6));
        if unit(fl.id) < 0.03 + 0.08 * cluster && fl.f1 < 0.4 {
            mineral = 2;
        }
        let boundary = smooth(0.12, 0.0, g.f2 - g.f1);
        let pick = smooth(0.7, 0.88, tile.value(p, 0.0022, sub(seed, 7)));
        let h = b.y * 0.0005 + lift[mineral] - pick * 0.00025 - boundary * 0.00004
            + (tile.value(p, 0.0012, sub(seed, 8)) - 0.5) * 0.00005;
        G {
            h,
            mineral,
            tint,
            boundary,
            pick,
        }
    });
    let height: Vec<f32> = px.iter().map(|g| g.h).collect();
    let r = relief(&tile, &height, &[0.0015, 0.005, 0.03], 1.5, 0.0015);
    let texels = tile.par(|_, x, y| {
        let i = y * tile.w + x;
        let g = px[i];
        let b = broad.at(x, y);
        let mut c = colors[g.mineral] * mix(0.9, 1.08, g.tint);
        c *= 1.0 - g.boundary * 0.08 + (white(i, 1) - 0.5) * 0.04;
        // Weathering: soft warm grime and a slow tone drift.
        c *= 1.0 + b.x * 0.08;
        let grime = smooth(0.0, 0.5, b.z);
        c = mix3(c, c * Vec3::new(0.8, 0.76, 0.7), grime * 0.45);
        c *= mix(0.6, 1.0, r.ao[i]) * (1.0 - smooth(0.0, 0.1, r.curv[i]) * 0.15);
        let ro = rough[g.mineral] + g.pick * 0.12 + grime * 0.08 + (white(i, 2) - 0.5) * 0.06;
        Texel::new(plausible(c), ro, r.ao[i])
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}

/// The dark basalt matrix shared by [`dark_stone`] and the rune inlays: height (m), albedo
/// and roughness at `p`.
pub(super) fn basalt(tile: &Tile, p: Vec2, i: usize, b: Vec4, seed: u32) -> (f32, Vec3, f32) {
    let mott = tile.fbm(p, 0.02, 2, 0.5, sub(seed, 14));
    let mut h =
        b.y * 0.0004 + mott * 0.00005 + (tile.value(p, 0.0015, sub(seed, 10)) - 0.5) * 0.00006;
    let mut c = hex(0x4a4643) * (1.0 + (tile.value(p, 0.002, sub(seed, 11)) - 0.5) * 0.2);
    c *= 1.0 + b.x * 0.2 + mott * 0.25 + (white(i, 3) - 0.5) * 0.06;
    // Fine pale feldspar microcrystals.
    let micro = tile.value(p, 0.0009, sub(seed, 15));
    c = mix3(c, hex(0x8a8680), smooth(0.8, 0.95, micro) * 0.35);
    let mut rough = 0.64 + b.z * 0.1 + mott * 0.05;
    // Plagioclase laths: a few small pale elongated crystals.
    let v = tile.voronoi(p, 0.008, 0.9, sub(seed, 12));
    let k = unit(v.id);
    if k < 0.08 {
        let a = unit(mix32(v.id)) * std::f32::consts::PI;
        let (s, co) = a.sin_cos();
        let q = Vec2::new(
            v.to_point.x * co + v.to_point.y * s,
            -v.to_point.x * s + v.to_point.y * co,
        );
        let lath = smooth(1.0, 0.6, (q.x / 0.3).powi(2) + (q.y / 0.08).powi(2));
        c = mix3(c, hex(0x77736c), lath * 0.6);
        rough = mix(rough, 0.4, lath);
    } else if k < 0.1 {
        // Olivine: glassy green-black grains.
        let o = smooth(0.28, 0.16, v.f1);
        c = mix3(c, hex(0x3a4128), o * 0.6);
        rough = mix(rough, 0.3, o);
    }
    // Vesicles: gas bubbles left as round pores.
    let ve = tile.voronoi(p, 0.014, 1.0, sub(seed, 13));
    if unit(ve.id) < 0.08 {
        let rr = 0.06 + 0.14 * unit(mix32(ve.id ^ 9));
        let pore = smooth(rr, rr * 0.6, ve.f1);
        h -= pore * 0.0012;
        c = mix3(c, hex(0x2a2826), pore * 0.6);
        rough = mix(rough, 0.9, pore);
    }
    // Weathered patina in broad patches.
    let pat = smooth(0.05, 0.5, b.w);
    c = mix3(c, hex(0x605950), pat * 0.4);
    rough += pat * 0.1;
    (h, c, rough)
}

/// Low-frequency layers for [`basalt`].
pub(super) fn basalt_layers(tile: &Tile, seed: u32) -> Coarse<Vec4> {
    tile.coarse(4, |p| {
        Vec4::new(
            tile.fbm(p, 0.3, 4, 0.5, sub(seed, 1)),
            tile.fbm(p, 0.15, 4, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.2, 3, 0.5, sub(seed, 3)),
            tile.fbm(p, 0.5, 4, 0.55, sub(seed, 4)),
        )
    })
}

/// Honed dark basalt for trims and inlays: fine dark matrix, pale feldspar laths, glassy
/// olivine, round vesicles and a faint weathered patina. 0.8 m per tile.
pub fn dark_stone(shrink: u32) -> Generated {
    let tile = Tile::new(512 >> shrink, 512 >> shrink, 0.8);
    let seed = 0xba5a_0001;
    let layers = basalt_layers(&tile, seed);
    let px = tile.par(|p, x, y| basalt(&tile, p, y * tile.w + x, layers.at(x, y), seed));
    let height: Vec<f32> = px.iter().map(|v| v.0).collect();
    let r = relief(&tile, &height, &[0.0015, 0.006, 0.03], 1.5, 0.0015);
    let texels = tile.par_idx(|i| {
        let (_, c, ro) = px[i];
        let c = c * 1.25 * mix(0.6, 1.0, r.ao[i]) * (1.0 + smooth(0.0, -0.1, r.curv[i]) * 0.25);
        Texel::new(plausible(c), ro.clamp(0.3, 1.0), r.ao[i])
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}

/// The two nearest fracture planes at a point (see [`facets`]).
pub(super) struct FacetPair {
    /// Heights (cell units) of the nearest and second-nearest cells' planes.
    pub planes: [f32; 2],
    /// Distance to the border between them (cell units).
    pub gap: f32,
    /// Hashes of the nearest and second-nearest cells.
    pub ids: [u32; 2],
}

impl FacetPair {
    /// Height with the step between the two planes eased over `width` cell units.
    pub fn height(&self, width: f32) -> f32 {
        let t = smooth(0.0, width, self.gap) * 0.5 + 0.5;
        mix(self.planes[1], self.planes[0], t)
    }

    /// A hash shared by both sides of the border.
    pub fn border(&self) -> u32 {
        mix32(self.ids[0] ^ self.ids[1])
    }
}

/// Voronoi cells, each a tilted plane with a random offset (`slope` cell units).
pub(super) fn facet_pair(p: Vec2, period: [i32; 2], slope: f32, seed: u32) -> FacetPair {
    let s = mix32(seed);
    let x = (p.x - floor(p.x)) * period[0] as f32;
    let y = (p.y - floor(p.y)) * period[1] as f32;
    let (ix, iy) = ((x as i32).min(period[0] - 1), (y as i32).min(period[1] - 1));
    let f = Vec2::new(x - ix as f32, y - iy as f32);
    // (squared distance, offset from the feature point, cell hash) of the two nearest.
    let mut best = [(f32::MAX, Vec2::ZERO, 0u32); 2];
    for j in -1..=1 {
        for i in -1..=1 {
            let cx = (ix + i).rem_euclid(period[0]);
            let cy = (iy + j).rem_euclid(period[1]);
            let h = hash2(cx, cy, s);
            let o = Vec2::new(i as f32 + unit(h), j as f32 + unit(mix32(h ^ 0x68e3_1da4)));
            let r = f - o;
            let d = r.length_squared();
            if d < best[0].0 {
                best[1] = best[0];
                best[0] = (d, r, h);
            } else if d < best[1].0 {
                best[1] = (d, r, h);
            }
        }
    }
    // Only the two nearest planes matter.
    let plane = |(_, r, h): (f32, Vec2, u32)| {
        let sl = Vec2::new(unit(mix32(h ^ 1)) - 0.5, unit(mix32(h ^ 2)) - 0.5) * 2.0 * slope;
        (unit(mix32(h ^ 3)) - 0.5) * slope + r.dot(sl)
    };
    FacetPair {
        planes: [plane(best[0]), plane(best[1])],
        gap: best[1].0.sqrt() - best[0].0.sqrt(),
        ids: [best[0].2, best[1].2],
    }
}

/// Faceted fracture planes: Voronoi cells, each a tilted plane; the boundary between the two
/// nearest cells becomes a ledge (`width` cell units wide). Returns (height in cell units, gap
/// to the next cell, cell id).
pub(super) fn facets(
    p: Vec2,
    period: [i32; 2],
    slope: f32,
    width: f32,
    seed: u32,
) -> (f32, f32, u32) {
    let f = facet_pair(p, period, slope, seed);
    (f.height(width), f.gap, f.ids[0])
}

/// Rough natural rock: eroded ridges and hollows, fracture facets at two sizes with ledges
/// and a few deep cracks, grain, and lichen, moss and dirt where they would grow. 2 m per
/// tile.
pub fn rock(shrink: u32) -> Generated {
    let tile = Tile::new(1024 >> shrink, 1024 >> shrink, 2.0);
    let seed = 0x40c4_0002;
    let big = tile.coarse(4, |p| {
        let q = tile.warp(p, 0.5, 0.08, 3, sub(seed, 11));
        Vec4::new(
            tile.ridged(q, 0.6, 5, 0.5, sub(seed, 1)),
            tile.fbm(p, 1.0, 3, 0.5, sub(seed, 2)),
            tile.fbm(p, 0.3, 4, 0.55, sub(seed, 3)),
            tile.fbm(p, 0.6, 3, 0.5, sub(seed, 4)),
        )
    });
    let mid = tile.coarse(2, |p| {
        let q = tile.warp(p, 0.2, 0.03, 2, sub(seed, 12));
        Vec4::new(
            tile.fbm(p, 0.08, 4, 0.55, sub(seed, 5)),
            tile.fbm(p, 0.04, 3, 0.5, sub(seed, 6)),
            tile.fbm(p, 0.15, 3, 0.5, sub(seed, 7)),
            tile.ridged(q, 0.15, 3, 0.5, sub(seed, 13)),
        )
    });
    let big_facets = tile.period(0.35);
    let small_facets = tile.period(0.12);
    #[derive(Clone, Copy, Default)]
    struct R {
        h: f32,
        crack: f32,
        facet: u32,
    }
    let px = tile.par(|p, x, y| {
        let b = big.at(x, y);
        let m = mid.at(x, y);
        let q = p + tile.off(Vec2::new(m.x, m.y) * 0.02);
        // Fracture faces: most borders are weathered into soft folds, some stay sharp
        // ledges, and a few have opened into cracks.
        let big_f = facet_pair(q, big_facets, 0.5, sub(seed, 8));
        let edge = unit(big_f.border());
        let f1 = big_f.height(if edge < 0.3 { 0.04 } else { 0.35 });
        let small_f = facet_pair(q, small_facets, 0.4, sub(seed, 9));
        let f2 = small_f.height(if unit(small_f.border()) < 0.3 {
            0.05
        } else {
            0.4
        });
        let gap = big_f.gap;
        let id = big_f.ids[0];
        let crack = if edge < 0.08 {
            smooth(0.012, 0.0, gap) * smooth(-0.3, 0.1, m.z)
        } else {
            0.0
        };
        let mut h = b.x * 0.05 + f1 * 0.35 * 0.45 + f2 * 0.12 * 0.3 + m.w * 0.008;
        h += m.x * 0.004 + m.y * 0.0015;
        // Granular weathered skin with small pits.
        let grain = tile.value(p, 0.003, sub(seed, 10));
        h += (grain - 0.5) * 0.0009 - smooth(0.82, 0.95, grain) * 0.0012;
        h += (white(y * tile.w + x, 9) - 0.5) * 0.00015;
        h -= crack * 0.012;
        R {
            h,
            crack,
            facet: id,
        }
    });
    let height: Vec<f32> = px.iter().map(|v| v.h).collect();
    let r = relief(&tile, &height, &[0.006, 0.02, 0.06, 0.2], 1.0, 0.006);
    let texels = tile.par(|p, x, y| {
        let i = y * tile.w + x;
        let v = px[i];
        let b = big.at(x, y);
        let m = mid.at(x, y);
        let ao = r.ao[i];
        let curv = r.curv[i];
        let convex = smooth(0.0, -0.12, curv);
        let concave = smooth(0.0, 0.15, curv);
        // Gray rock drifting between cool gray and warm brown, per-facet shifts.
        let warm = smooth(-0.3, 0.4, b.y);
        let mut c = mix3(hex(0x7a7874), hex(0x83725f), warm);
        c *= mix(0.88, 1.1, unit(mix32(v.facet ^ 5))) * (1.0 + m.z * 0.15);
        // Mineral flecks.
        let fleck = tile.value(p, 0.0022, sub(seed, 14));
        c *= 1.0 - smooth(0.75, 0.9, fleck) * 0.3 + smooth(0.25, 0.1, fleck) * 0.18;
        c *= 1.0 + (white(i, 1) - 0.5) * 0.08;
        // Iron staining in hollows.
        let iron = smooth(0.1, 0.5, b.w) * (0.3 + concave);
        c = mix3(c, c * Vec3::new(1.15, 0.85, 0.6), iron.min(1.0) * 0.5);
        // Weathered rind on convex edges, grime in hollows.
        c *= 1.0 + convex * 0.18 - concave * 0.2;
        c *= mix(0.4, 1.0, ao);
        let mut rough = 0.85 - convex * 0.06 + concave * 0.05 + (white(i, 2) - 0.5) * 0.06;
        // Dirt in cracks and deep hollows.
        let dirt = (v.crack + smooth(0.55, 0.25, ao) * 0.7).min(1.0);
        c = mix3(c, hex(0x3b3128) * mix(0.6, 1.0, ao), dirt * 0.75);
        // Moss in sheltered hollows.
        let moss = smooth(0.15, 0.45, b.z) * smooth(0.85, 0.55, ao + convex) * 0.9;
        let moss_c = mix3(
            hex(0x3b4a1e),
            hex(0x5e6a26),
            tile.value(p, 0.004, sub(seed, 15)),
        );
        c = mix3(c, moss_c * mix(0.5, 1.0, ao), moss);
        // Lichen crusts on exposed tops.
        let lich_n = tile.value(p, 0.012, sub(seed, 16)) + m.z * 0.6;
        let lichen = (smooth(0.55, 0.75, lich_n)
            * smooth(-0.1, 0.4, b.z)
            * (0.3 + convex)
            * smooth(0.6, 0.9, ao))
        .min(1.0);
        let crust = smooth(0.3, 0.6, tile.value(p, 0.002, sub(seed, 17)));
        let lc = if b.w > 0.15 {
            hex(0xb49c55)
        } else {
            hex(0xa6a893)
        };
        c = mix3(c, lc * mix(0.7, 1.05, crust), lichen * 0.85);
        rough = mix(rough, 0.95, moss.max(lichen)) + dirt * 0.05;
        Texel::new(plausible(c), rough, ao)
    });
    let maps = Maps::new(tile, r.normal, &texels, false, false);
    Generated::new(maps, StandardMaterial::default())
}
