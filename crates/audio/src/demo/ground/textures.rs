//! Texture recipes: one tileable PBR set (albedo with a coverage pattern in alpha, normal,
//! occlusion/roughness/metal) per ground and prop material of the trail. Each recipe paints
//! a height field and colours, mostly from noise plus thousands of small stamps (grass
//! blades, pebbles, straw, leaves, grains), and the canvas bakes the normal and cavity maps.

use std::f32::consts::TAU;

use engine_core::math::{Vec2, Vec3};

use super::procgen::{
    Canvas, Rand, TexSet, Texel, fbm, fbm2, hex, lerp, perlin_p, smooth, unit, worley,
};

/// Paints one texture set.
pub(super) type Recipe = fn() -> TexSet;

/// Every texture set by name, with its recipe.
pub(super) const RECIPES: &[(&str, Recipe)] = &[
    ("verge", verge),
    ("dirt", dirt),
    ("mud", mud),
    ("snow", snow),
    ("ice", ice),
    ("gravel", gravel),
    ("concrete", concrete_floor),
    ("formwork", formwork),
    ("sand", sand),
    ("forest", forest_floor),
    ("planks", planks),
    ("zinc", galvanized),
    ("tread", tread),
    ("leaf", leaf_atlas),
    ("stone", stone),
];

/// The ragged-edge coverage pattern every ground shares in its alpha: 0.5..1, with blobs
/// a few decimeters across (see the ribbon builder for how it is used).
fn coverage(u: f32, v: f32, freq: i32, seed: u32) -> f32 {
    let n = fbm(u, v, freq, 3, seed);
    0.5 + 0.5 * (0.5 + 0.9 * n).clamp(0.0, 1.0)
}

/// A thin tapered line `len` pixels long and `width` wide at its root, at `angle`.
/// `f(coverage, t along 0..1, texel)`.
#[allow(clippy::too_many_arguments)]
fn line(
    c: &mut Canvas,
    u: f32,
    v: f32,
    angle: f32,
    len: f32,
    width: f32,
    taper: f32,
    mut f: impl FnMut(f32, f32, &mut Texel),
) {
    let dir = Vec2::new(angle.cos(), angle.sin());
    let r = len * 0.5 + width + 1.5;
    c.stamp(u, v, r, |dx, dy, t| {
        let p = Vec2::new(dx, dy);
        let a = p.dot(dir);
        if a.abs() > len * 0.5 + 1.0 {
            return;
        }
        let along = (a / len + 0.5).clamp(0.0, 1.0);
        let d = (p - dir * a.clamp(-len * 0.5, len * 0.5)).length();
        let w = (width * 0.5 * (1.0 - taper * along)).max(0.3);
        let cov = (w + 0.5 - d).clamp(0.0, 1.0);
        if cov > 0.0 {
            f(cov, along, t);
        }
    });
}

/// An ellipse with radii `rx`, `ry` pixels turned by `angle`; `f(coverage, r 0..1 from the
/// centre, local position in radii, texel)`.
#[allow(clippy::too_many_arguments)]
fn ellipse(
    c: &mut Canvas,
    u: f32,
    v: f32,
    rx: f32,
    ry: f32,
    angle: f32,
    mut f: impl FnMut(f32, f32, Vec2, &mut Texel),
) {
    let (s, co) = angle.sin_cos();
    let rmax = rx.max(ry) + 1.5;
    c.stamp(u, v, rmax, |dx, dy, t| {
        let lx = (dx * co + dy * s) / rx;
        let ly = (-dx * s + dy * co) / ry;
        let r = (lx * lx + ly * ly).sqrt();
        let edge = rx.min(ry);
        let cov = ((1.0 - r) * edge + 0.5).clamp(0.0, 1.0);
        if cov > 0.0 {
            f(cov, r.min(1.0), Vec2::new(lx, ly), t);
        }
    });
}

fn mix(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a.lerp(b, t.clamp(0.0, 1.0))
}

// --------------------------------------------------------------------------- grounds

/// Grass verge: soil under a mat of flattened blades, thatch and clover.
fn verge() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(1.5));
    let (soil, soil_dark, moss) = (hex(0x4b3e30), hex(0x30281f), hex(0x545b30));
    c.fill(|u, v| {
        let n = fbm(u, v, 6, 4, 11);
        let m = fbm(u, v, 3, 2, 12);
        Texel {
            color: mix(soil, soil_dark, 0.5 + 0.35 * n).lerp(moss, smooth(-0.1, 0.5, m) * 0.25),
            height: n * 0.003,
            rough: 0.9,
            alpha: coverage(u, v, 5, 13),
            ..Default::default()
        }
    });
    let mut rng = Rand::new(1);
    let greens = [
        hex(0x637a3a),
        hex(0x546a30),
        hex(0x738a44),
        hex(0x7e8f4c),
        hex(0x45583a),
        hex(0x5f7640),
    ];
    let straw = [hex(0x9c8c58), hex(0x857447), hex(0xa89a66)];
    for _ in 0..13_000 {
        let (u, v) = (rng.f(), rng.f());
        // Patches of dead thatch.
        let dead = fbm(u, v, 4, 2, 77) > 0.4 && rng.chance(0.4) || rng.chance(0.06);
        let col = if dead {
            straw[rng.below(3)]
        } else {
            greens[rng.below(6)]
        } * rng.range(0.8, 1.15);
        let (ang, len, w) = (
            rng.range(0.0, TAU),
            rng.range(6.0, 18.0),
            rng.range(1.3, 2.4),
        );
        line(&mut c, u, v, ang, len, w, 0.8, |cov, along, t| {
            let tip = 0.75 + 0.35 * along;
            t.color = mix(t.color, col * tip, cov);
            t.height += 0.0011 * cov;
            t.rough = lerp(t.rough, if dead { 0.9 } else { 0.8 }, cov);
        });
    }
    // Clover and small round leaves.
    for _ in 0..120 {
        let (u, v) = (rng.f(), rng.f());
        let r = rng.range(2.0, 3.5);
        let rot = rng.range(0.0, TAU);
        let col = hex(0x5c6e3a) * rng.range(0.8, 1.0);
        for k in 0..3 {
            let a = rot + k as f32 * TAU / 3.0;
            let off = Vec2::new(a.cos(), a.sin()) * r * 0.9 / 512.0;
            ellipse(
                &mut c,
                u + off.x,
                v + off.y,
                r,
                r * 0.85,
                a,
                |cov, q, _, t| {
                    t.color = mix(t.color, col * (1.05 - 0.25 * q), cov);
                    t.height += 0.0015 * cov * (1.0 - q * q);
                    t.rough = lerp(t.rough, 0.88, cov);
                },
            );
        }
    }
    c.bake(0.9, 15.0)
}

/// Packed bare earth: the worn line down the middle of the grass.
fn dirt() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(1.5));
    let (light, dark, grey) = (hex(0x786d60), hex(0x524940), hex(0x77716b));
    c.fill(|u, v| {
        let n = fbm(u, v, 5, 4, 21);
        let f = fbm(u, v, 40, 2, 22);
        // Cracks wander (a warped cell net), are hairline thin and only open where the earth
        // has dried out, in patches.
        let (wu, wv) = (0.25 * fbm(u, v, 6, 2, 27), 0.25 * fbm(u, v, 6, 2, 28));
        let cell = worley((u + wu / 14.0) * 14.0, (v + wv / 14.0) * 14.0, 14, 23, 0.9);
        let dry = smooth(0.2, 0.6, fbm(u, v, 3, 2, 26));
        let crack = (1.0 - smooth(0.0, 0.018, cell.f2 - cell.f1)) * dry;
        let grit = unit(super::procgen::hash(
            (u * 512.0) as i32,
            (v * 512.0) as i32,
            24,
        ));
        let col = mix(light, dark, 0.45 + 0.5 * n).lerp(grey, smooth(0.2, 0.7, f) * 0.3)
            * (0.9 + 0.2 * grit)
            * (1.0 - crack * 0.15);
        Texel {
            color: col,
            height: n * 0.004 + f * 0.0008 - crack * 0.0007,
            rough: 0.92 - 0.1 * smooth(0.0, 1.0, n),
            alpha: coverage(u, v, 6, 25),
            ..Default::default()
        }
    });
    let mut rng = Rand::new(2);
    let pebbles = [hex(0x8f877a), hex(0x6d655a), hex(0xa39684), hex(0x5c5248)];
    for _ in 0..300 {
        let (u, v) = (rng.f(), rng.f());
        let rx = rng.range(1.2, 3.8);
        let col = pebbles[rng.below(4)] * rng.range(0.75, 1.05);
        let lift = rx * c.texel_m() * 0.5;
        ellipse(
            &mut c,
            u,
            v,
            rx,
            rx * rng.range(0.6, 1.0),
            rng.range(0.0, TAU),
            |cov, q, p, t| {
                let dome = (1.0 - q * q).max(0.0).sqrt();
                t.color = mix(t.color, col * (0.85 + 0.25 * (-p.y).max(0.0)), cov * 0.75);
                t.height = lerp(t.height, t.height + lift * dome, cov);
                t.rough = lerp(t.rough, 0.7, cov);
            },
        );
    }
    for _ in 0..700 {
        let (u, v) = (rng.f(), rng.f());
        let col = hex(0x8d7c52) * rng.range(0.7, 1.1);
        line(
            &mut c,
            u,
            v,
            rng.range(0.0, TAU),
            rng.range(5.0, 16.0),
            rng.range(1.0, 1.8),
            0.6,
            |cov, _, t| {
                t.color = mix(t.color, col, cov * 0.85);
                t.height += 0.0006 * cov;
            },
        );
    }
    c.bake(1.3, 30.0)
}

/// Wet mud: damp hollows with a sheen and films of water in the lowest, matte lumpy clods
/// and drier ridges, straw and grit.
fn mud() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(2.0));
    let (wet, dry, brown) = (hex(0x27231f), hex(0x5a5047), hex(0x403830));
    c.fill(|u, v| {
        let n = fbm(u, v, 4, 5, 31);
        let d = fbm(u, v, 12, 3, 32);
        let s = fbm2(u, v, 9, 3, 3, 33);
        let h = n * 0.008 + d * 0.0025 + s * 0.0015;
        // Low places are damp and darker, a soft sheen rather than a gloss; the lowest
        // hold a film of water, flat and glassy, that lies over the mud's bumps.
        let low = n + d * 0.3;
        let wetness = smooth(0.0, -0.4, low);
        let film = smooth(-0.42, -0.5, low);
        let col = mix(brown, dry, smooth(-0.2, 0.6, n + d * 0.5)).lerp(wet, wetness * 0.8);
        // The films lie flat over the bumps, so only they mirror the sky.
        let height = (h - wetness * 0.002).max(-0.0056);
        Texel {
            color: col * (1.0 - 0.2 * film),
            height: lerp(height, -0.0056, smooth(0.3, 0.8, film)),
            rough: lerp(lerp(0.88, 0.62, wetness), 0.08, film),
            alpha: coverage(u, v, 5, 34),
            ..Default::default()
        }
    });
    let mut rng = Rand::new(3);
    // Squelched clods, lumpy and drier on top, and grit.
    for _ in 0..450 {
        let (u, v) = (rng.f(), rng.f());
        let r = rng.range(2.0, 7.0);
        let col = hex(0x4b433a) * rng.range(0.85, 1.3);
        let lift = rng.range(0.0008, 0.0028);
        let (k1, k2) = (rng.range(0.0, TAU), rng.range(0.0, TAU));
        ellipse(
            &mut c,
            u,
            v,
            r,
            r * rng.range(0.5, 1.0),
            rng.range(0.0, TAU),
            |cov, q, p, t| {
                let a = p.y.atan2(p.x);
                let lobes = 1.0 + 0.22 * (3.0 * a + k1).sin() + 0.12 * (5.0 * a + k2).sin();
                let cov = cov * smooth(1.0, 0.8, q * lobes);
                let dome = (1.0 - q * q).max(0.0);
                t.height += lift * dome * cov;
                t.color = mix(t.color, col, cov * 0.55);
                t.rough = lerp(t.rough, 0.88, cov * 0.7);
            },
        );
    }
    for _ in 0..80 {
        let (u, v) = (rng.f(), rng.f());
        let col = hex(0x7a6c4e) * rng.range(0.5, 0.9);
        line(
            &mut c,
            u,
            v,
            rng.range(0.0, TAU),
            rng.range(10.0, 30.0),
            rng.range(1.3, 2.2),
            0.2,
            |cov, _, t| {
                t.color = mix(t.color, col, cov * 0.7);
                t.height += 0.0006 * cov;
                t.rough = lerp(t.rough, 0.55, cov);
            },
        );
    }
    for _ in 0..140 {
        let (u, v) = (rng.f(), rng.f());
        let r = rng.range(1.0, 2.6);
        let col = hex(0x5f574d) * rng.range(0.7, 1.15);
        ellipse(
            &mut c,
            u,
            v,
            r,
            r * 0.8,
            rng.range(0.0, TAU),
            |cov, q, _, t| {
                t.color = mix(t.color, col, cov);
                t.height += 0.0012 * (1.0 - q * q) * cov;
                t.rough = lerp(t.rough, 0.5, cov);
            },
        );
    }
    c.bake(1.2, 26.0)
}

/// Settled snow: soft drifts and wind ripples, a granular crust of old crystals, and ice
/// facets tilted every which way that glint in the sun; hollows a little blue. Albedo of a
/// few days' old snow (about 0.8), so a sunlit field doesn't burn out to flat white.
fn snow() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(2.0));
    let (white, blue) = (Vec3::new(0.76, 0.79, 0.84), Vec3::new(0.52, 0.61, 0.77));
    c.fill(|u, v| {
        let n = fbm(u, v, 3, 5, 41);
        let ripple = fbm2(u, v, 2, 14, 3, 42);
        let grain = fbm(u, v, 64, 2, 43);
        // Rounded old grains a few millimeters across.
        let crust = worley(u * 160.0, v * 160.0, 160, 46, 1.0);
        let bead = (1.0 - crust.f1 * 1.6).max(0.0);
        let crusty = smooth(-0.2, 0.5, fbm(u, v, 6, 2, 47));
        let h = n * 0.006 + ripple * 0.0016 + grain * 0.0004 + bead * 0.0007 * crusty;
        let hollow = smooth(0.0, -0.6, n + ripple * 0.3);
        let px = super::procgen::hash((u * 512.0) as i32, (v * 512.0) as i32, 44);
        let sparkle = unit(px) > 0.985;
        let facet = if sparkle {
            let a = unit(px >> 3) * TAU;
            let tilt = 0.2 + 0.45 * unit(px >> 11);
            Vec2::new(a.cos(), a.sin()) * tilt
        } else {
            Vec2::ZERO
        };
        Texel {
            color: if sparkle {
                Vec3::splat(0.9)
            } else {
                mix(white, blue, hollow * 0.6)
                    * (0.96 + 0.05 * grain)
                    * (1.0 - 0.04 * crusty * (1.0 - bead))
            },
            height: h,
            rough: if sparkle {
                0.14
            } else {
                0.6 + 0.12 * grain + 0.06 * crusty
            },
            // Crumbly: blobs of a few centimeters broken up by the grains, so thin snow
            // and the snow's edge fall apart into crumbs rather than smooth lobes.
            alpha: 0.5
                + 0.5
                    * (0.5
                        + 0.9 * fbm(u, v, 10, 3, 45)
                        + 0.45 * (bead - 0.5)
                        + 0.3 * fbm(u, v, 40, 2, 48))
                    .clamp(0.0, 1.0),
            facet,
            ..Default::default()
        }
    });
    c.bake(1.0, 6.0)
}

/// Trodden path ice: grey and milky where puddles froze over the path, clear and dark in
/// black-ice pools that show the ground under them, frosted and dusted with blown snow in
/// patches, scuffed white by boots, with cracks, trapped bubbles and grit.
fn ice() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(2.5));
    let (clear, milky, frost, crack) = (
        Vec3::new(0.075, 0.095, 0.105),
        Vec3::new(0.38, 0.45, 0.5),
        Vec3::new(0.76, 0.8, 0.85),
        Vec3::new(0.64, 0.72, 0.78),
    );
    c.fill(|u, v| {
        let n = fbm(u, v, 3, 4, 51);
        let big = worley(u * 5.0 + 0.3 * n, v * 5.0 - 0.3 * n, 5, 52, 1.0);
        let small = worley(u * 13.0 + 0.2 * n, v * 13.0, 13, 53, 1.0);
        // Cracks fade in and out along their length; the fine net only in places.
        let fade = smooth(-0.3, 0.3, fbm(u, v, 9, 2, 56));
        let c1 = (1.0 - smooth(0.0, 0.014, big.f2 - big.f1)) * (0.25 + 0.75 * fade);
        let c2 = (1.0 - smooth(0.0, 0.018, small.f2 - small.f1)) * smooth(0.0, 0.45, n) * fade;
        let cracks = c1.max(c2 * 0.5);
        // Clear black ice in pools; elsewhere milky layers, cloudier in places.
        let pool = smooth(0.1, 0.4, fbm(u, v, 4, 4, 57));
        let milk = 0.5 + 0.5 * fbm(u, v, 6, 4, 54);
        // Frost and blown snow in drifts away from the pools, grainy at their edges.
        let grain = fbm(u, v, 48, 2, 59);
        let dust = smooth(0.18, 0.6, fbm(u, v, 7, 4, 58) + 0.12 * grain) * (1.0 - pool);
        let body = (milky * (0.8 + 0.3 * milk)).lerp(clear, pool * 0.9);
        let col = body
            .lerp(crack, cracks * 0.5 * (1.0 - pool * 0.4))
            .lerp(frost * (0.96 + 0.06 * grain), dust * 0.9);
        Texel {
            color: col,
            height: n * 0.0012 - cracks * 0.0008 + dust * (0.0005 + 0.0003 * grain),
            rough: lerp(
                lerp(lerp(0.16, 0.3, milk), 0.05, pool).max(cracks * 0.45),
                0.72,
                dust,
            ),
            alpha: coverage(u, v, 5, 55),
            ..Default::default()
        }
    });
    let mut rng = Rand::new(5);
    // Hairline cracks.
    for _ in 0..40 {
        let (u, v) = (rng.f(), rng.f());
        let len = rng.range(20.0, 100.0);
        line(
            &mut c,
            u,
            v,
            rng.range(0.0, TAU),
            len,
            1.0,
            0.0,
            |cov, _, t| {
                t.color = mix(t.color, crack * 0.9, cov * 0.45);
                t.height -= 0.0003 * cov;
                t.rough = lerp(t.rough, 0.35, cov);
            },
        );
    }
    // Boot scuffs: bundles of short parallel scratches, white and matte.
    for _ in 0..110 {
        let (cu, cv) = (rng.f(), rng.f());
        let a = rng.range(0.0, TAU);
        let side = Vec2::new(-a.sin(), a.cos()) / 512.0;
        let len = rng.range(14.0, 55.0);
        let k = rng.below(6) + 3;
        let spread = rng.range(0.8, 2.2);
        let white = rng.range(0.45, 0.8);
        for i in 0..k {
            let off = side * (i as f32 - k as f32 * 0.5) * spread;
            let shift = rng.range(-0.25, 0.25) * len / 512.0;
            let dir = Vec2::new(a.cos(), a.sin()) * shift;
            let l = len * rng.range(0.5, 1.0);
            let w = rng.range(0.7, 1.5);
            line(
                &mut c,
                cu + off.x + dir.x,
                cv + off.y + dir.y,
                a + rng.normal(0.04),
                l,
                w,
                0.6,
                |cov, _, t| {
                    t.color = mix(t.color, frost, cov * white);
                    t.height -= 0.00015 * cov;
                    t.rough = lerp(t.rough, 0.62, cov * 0.9);
                },
            );
        }
    }
    // Grit and bits of the path frozen in or blown on.
    for _ in 0..1500 {
        let (u, v) = (rng.f(), rng.f());
        let r = rng.range(0.5, 1.6);
        let col = [hex(0x3b3128), hex(0x5a4e42), hex(0x6e6a64)][rng.below(3)];
        let lift = rng.range(0.0, 0.0004);
        ellipse(
            &mut c,
            u,
            v,
            r,
            r * rng.range(0.6, 1.0),
            rng.range(0.0, TAU),
            |cov, q, _, t| {
                t.color = mix(t.color, col, cov * 0.85);
                t.height += lift * (1.0 - q * q) * cov;
                t.rough = lerp(t.rough, 0.8, cov);
            },
        );
    }
    // Trapped bubbles in clusters.
    for _ in 0..60 {
        let (cu, cv) = (rng.f(), rng.f());
        let spread = rng.range(4.0, 20.0) / 512.0;
        for _ in 0..rng.below(14) + 3 {
            let (u, v) = (cu + rng.normal(spread), cv + rng.normal(spread));
            let r = rng.range(0.8, 3.2);
            ellipse(
                &mut c,
                u,
                v,
                r,
                r * rng.range(0.7, 1.0),
                rng.range(0.0, TAU),
                |cov, q, p, t| {
                    let rim = smooth(0.55, 0.95, q);
                    let lit = 0.75 + 0.25 * (-p.y);
                    t.color = mix(
                        t.color,
                        Vec3::new(0.7, 0.8, 0.85) * lit * (0.6 + 0.6 * rim),
                        cov * 0.6,
                    );
                    t.rough = lerp(t.rough, 0.15, cov);
                },
            );
        }
    }
    c.bake(1.0, 10.0)
}

/// A bed of fine gravel: rounded pebbles of mixed sizes packed in dusty fines.
fn gravel() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(1.0));
    let palette = [
        hex(0x8d877d),
        hex(0x6f6a62),
        hex(0xa59d8f),
        hex(0x7b746c),
        hex(0x5e5a56),
        hex(0x9a8a74),
        hex(0x8a7f78),
        hex(0xb3aa9a),
    ];
    let gap = hex(0x3a332c);
    c.fill(|u, v| {
        let warp = fbm(u, v, 8, 2, 61) * 0.4;
        let a = worley(u * 38.0 + warp, v * 38.0 - warp, 38, 62, 0.95);
        let b = worley(u * 85.0, v * 85.0, 85, 63, 0.95);
        let edge_a = smooth(0.0, 0.28, a.f2 - a.f1);
        let edge_b = smooth(0.0, 0.25, b.f2 - b.f1);
        let id = a.id;
        let base = palette[(id >> 3) as usize % palette.len()] * (0.8 + 0.4 * unit(id ^ 0x77));
        let speck = fbm(u, v, 160, 1, 64);
        let stone = base * (0.92 + 0.16 * speck);
        let small = palette[(b.id >> 5) as usize % palette.len()] * 0.8;
        // Rounded pebbles of their own size, packed against their neighbours, with fines
        // showing between.
        let r = 0.34 + 0.3 * unit(id >> 12);
        let round = (1.0 - (a.f1 / r).powi(2)).max(0.0).sqrt();
        let dome = round.min(edge_a.sqrt() * 1.5).min(1.0);
        let fine = edge_b.sqrt() * 0.4;
        let col = if dome > 0.15 {
            mix(small, stone, smooth(0.15, 0.45, dome))
        } else {
            mix(gap, small, fine * 1.6)
        };
        let h = 0.008 * dome * (0.7 + 0.5 * unit(id >> 4)) + 0.002 * fine;
        Texel {
            color: col,
            height: h,
            rough: lerp(0.9, 0.62 + 0.2 * unit(id >> 9), dome),
            alpha: coverage(u, v, 4, 65),
            ..Default::default()
        }
    });
    c.bake(1.4, 70.0)
}

/// Cast concrete: fine aggregate, pores, blotchy stains.
fn concrete_floor() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(2.0));
    let (base, dark, green) = (hex(0x9a958c), hex(0x6e6a63), hex(0x6f725e));
    c.fill(|u, v| {
        let n = fbm(u, v, 4, 5, 71);
        let stain = smooth(0.15, 0.6, fbm(u, v, 3, 4, 72));
        let agg = worley(u * 160.0, v * 160.0, 160, 73, 0.9);
        let stone = smooth(0.08, 0.25, agg.f2 - agg.f1) * smooth(0.6, 0.9, unit(agg.id));
        let grit = fbm(u, v, 128, 1, 74);
        let col = mix(base, dark, 0.35 + 0.3 * n).lerp(green, stain * 0.35)
            * (0.95 + 0.06 * grit + 0.06 * stone);
        Texel {
            color: col,
            height: n * 0.0008 + grit * 0.0002 + stone * 0.0002,
            rough: 0.82 - 0.12 * stain,
            alpha: coverage(u, v, 5, 75),
            ..Default::default()
        }
    });
    let mut rng = Rand::new(7);
    // Air voids.
    for _ in 0..1800 {
        let (u, v) = (rng.f(), rng.f());
        let r = rng.range(0.6, 1.8);
        ellipse(&mut c, u, v, r, r, 0.0, |cov, q, _, t| {
            t.height -= 0.0006 * (1.0 - q * q) * cov;
            t.color *= 1.0 - 0.2 * cov;
        });
    }
    // Hairline shrinkage cracks that wander.
    for _ in 0..14 {
        let (mut u, mut v) = (rng.f(), rng.f());
        let mut a = rng.range(0.0, TAU);
        for _ in 0..rng.below(6) + 3 {
            let len = rng.range(10.0, 30.0);
            let (cu, cv) = (u + a.cos() * len / 1024.0, v + a.sin() * len / 1024.0);
            line(&mut c, cu, cv, a, len, 1.0, 1.0, |cov, _, t| {
                t.color *= 1.0 - 0.3 * cov;
                t.height -= 0.0003 * cov;
                t.rough = lerp(t.rough, 0.95, cov);
            });
            u += a.cos() * len / 512.0;
            v += a.sin() * len / 512.0;
            a += rng.normal(0.5);
        }
    }
    // Dark drips and old spills.
    for _ in 0..30 {
        let (u, v) = (rng.f(), rng.f());
        let r = rng.range(6.0, 22.0);
        let k = rng.range(0.08, 0.2);
        ellipse(
            &mut c,
            u,
            v,
            r,
            r * rng.range(0.5, 1.0),
            rng.range(0.0, TAU),
            |cov, q, _, t| {
                t.color *= 1.0 - k * cov * (1.0 - q * q * 0.5);
                t.rough = lerp(t.rough, 0.6, cov * 0.5);
            },
        );
    }
    c.bake(1.2, 50.0)
}

/// Board-formed concrete walls, 3 m square: horizontal board marks with grain, fins at the
/// joints, tie holes, rain streaks from the top and a dirty splash zone at the bottom.
fn formwork() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(3.0));
    let (base, dark, algae) = (hex(0x9b978e), hex(0x605c55), hex(0x55583f));
    let boards = 20.0;
    c.fill(|u, v| {
        let b = (v * boards).floor();
        let fb = v * boards - b;
        let id = super::procgen::hash(b as i32, 0, 81);
        let step = (unit(id) - 0.5) * 0.0024;
        let tone = (unit(id >> 3) - 0.5) * 0.12;
        let grain = perlin_p(u * 6.0, v * 160.0, 6, 160, 82) * 0.5
            + perlin_p(u * 24.0, v * 640.0, 24, 640, 83) * 0.25;
        let fin = 1.0 - smooth(0.0, 0.035, fb.min(1.0 - fb));
        let n = fbm(u, v, 4, 4, 84);
        // Rain streaks: stretched noise, strongest near the top (v = 0).
        let streak = smooth(0.1, 0.7, fbm2(u, v, 24, 1, 3, 85)) * (1.0 - v).powf(0.7);
        let splash = smooth(0.82, 1.0, v) * (0.6 + 0.4 * fbm(u, v, 12, 2, 86));
        let col = (base * (1.0 + tone + 0.05 * grain) * (0.96 + 0.06 * n))
            .lerp(dark, streak * 0.45 + splash * 0.35)
            .lerp(algae, splash * 0.3);
        Texel {
            color: col,
            height: step + grain * 0.0004 + fin * 0.0012 + n * 0.0006,
            rough: 0.86 - 0.1 * streak,
            ..Default::default()
        }
    });
    // Tie holes on a grid.
    for i in 0..4 {
        for j in 0..3 {
            let (u, v) = ((i as f32 + 0.5) / 4.0, (j as f32 + 0.5) / 3.0);
            ellipse(&mut c, u, v, 4.5, 4.5, 0.0, |cov, q, _, t| {
                let ring = smooth(0.6, 0.85, q);
                t.height -= 0.006 * cov * (1.0 - ring);
                t.color = mix(t.color, hex(0x55524d), cov * (1.0 - ring * 0.6));
            });
        }
    }
    c.bake(1.3, 30.0)
}

/// Fine sand in wind ripples, with heavier dark grains in the troughs and shell grit.
fn sand() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(1.5));
    let (light, warm, dark) = (hex(0xd8c39a), hex(0xc4a676), hex(0x6b5a48));
    c.fill(|u, v| {
        let warp =
            fbm(u, v, 2, 3, 91) * 2.6 + fbm(u, v, 5, 2, 92) * 0.9 + fbm(u, v, 11, 2, 96) * 0.25;
        let phase = v * 18.0 + warp;
        // Ripples fade out where the sand was trampled or sheltered.
        let amp = 0.25 + 0.75 * smooth(-0.35, 0.25, fbm(u, v, 3, 2, 97));
        let f = phase - phase.floor();
        // Gentle windward slope, steep lee face.
        let profile = if f < 0.72 {
            smooth(0.0, 0.72, f)
        } else {
            1.0 - smooth(0.72, 1.0, f)
        };
        let px = super::procgen::hash((u * 512.0) as i32, (v * 512.0) as i32, 93);
        let g = unit(px);
        let dark_grain = g > 0.96 || (g > 0.9 && profile < 0.25);
        let white = unit(px >> 5) > 0.985;
        let n = fbm(u, v, 6, 3, 94);
        let profile = profile * amp;
        let mut col = mix(light, warm, 0.5 + 0.5 * n) * (0.94 + 0.08 * profile);
        col *= 0.93 + 0.1 * unit(px >> 11);
        if dark_grain {
            col = dark * (0.8 + 0.4 * unit(px >> 3));
        } else if white {
            col = Vec3::splat(0.85);
        }
        Texel {
            color: col,
            height: profile * 0.003 + n * 0.0025,
            rough: 0.95,
            // Coarse lobes broken down to drifts of grains, so the thin edge dissolves
            // instead of ending in a cut-out line.
            alpha: 0.5
                + 0.5
                    * (0.5
                        + 0.8 * fbm(u, v, 5, 3, 95)
                        + 0.45 * fbm(u, v, 24, 2, 98)
                        + 0.35 * (unit(px >> 17) - 0.5))
                        .clamp(0.0, 1.0),
            ..Default::default()
        }
    });
    let mut rng = Rand::new(9);
    for _ in 0..400 {
        let (u, v) = (rng.f(), rng.f());
        let r = rng.range(1.0, 3.0);
        let col = [hex(0xf0e8dc), hex(0xd9b8a0), hex(0x9a8e80)][rng.below(3)];
        ellipse(
            &mut c,
            u,
            v,
            r,
            r * 0.6,
            rng.range(0.0, TAU),
            |cov, _, _, t| {
                t.color = mix(t.color, col, cov);
                t.rough = lerp(t.rough, 0.5, cov);
                t.height += 0.0004 * cov;
            },
        );
    }
    c.bake(0.85, 15.0)
}

/// Forest floor: dark soil under decayed leaves (real leaf outlines, overlapping, each
/// shading the one under it, some curled up at an edge), needles, twigs and moss. The
/// coverage alpha follows the leaves, so where the litter thins out at its edges it falls
/// apart leaf by leaf.
fn forest_floor() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(2.0));
    let (soil, moss) = (hex(0x2c2420), hex(0x464f2c));
    c.fill(|u, v| {
        let n = fbm(u, v, 5, 4, 101);
        let m = smooth(0.3, 0.7, fbm(u, v, 4, 3, 102));
        Texel {
            color: soil * (0.85 + 0.3 * n) + moss * m * 0.5,
            height: n * 0.003,
            rough: 0.85,
            // Bare soil between the leaves is the first to go at a ragged edge.
            alpha: 0.5 + 0.6 * (coverage(u, v, 5, 103) - 0.5),
            ..Default::default()
        }
    });
    let mut rng = Rand::new(11);
    let leaf_cols = [
        hex(0x55402c),
        hex(0x6e5238),
        hex(0x3a2b1e),
        hex(0x7a5a36),
        hex(0x5e4a36),
        hex(0x857052),
        hex(0x46382a),
        hex(0x2a2119),
        hex(0x7d6a4a),
    ];
    for _ in 0..2300 {
        let (u, v) = (rng.f(), rng.f());
        // Leaf length in pixels (5..12 cm).
        let size = rng.range(13.0, 30.0);
        let ang = rng.range(0.0, TAU);
        let cell = rng.below(4) as u32;
        let col = leaf_cols[rng.below(leaf_cols.len())] * rng.range(0.75, 1.15);
        // Older leaves lose their colour to grey-brown.
        let col = col.lerp(
            Vec3::splat(col.dot(Vec3::splat(1.0 / 3.0))),
            rng.range(0.0, 0.5),
        );
        let curl = rng.range(-1.0, 1.0);
        let (sn, cs) = ang.sin_cos();
        // Leaf space of a texel `(dx, dy)` pixels from the centre: across and along.
        let local = move |dx: f32, dy: f32| {
            let along = (dx * cs + dy * sn) / size;
            let across = (-dx * sn + dy * cs) / size;
            (across, along)
        };
        // Its shadow on what lies under it, offset away from the light.
        c.stamp(u, v, size * 0.6 + 2.0, |dx, dy, t| {
            let (cc, a) = local(dx - 1.5, dy - 1.5);
            let (inside, _) = leaf_shape(cell, cc, a);
            let k = smooth(-0.02, 0.03, inside);
            t.color *= 1.0 - 0.22 * k;
            t.ao *= 1.0 - 0.15 * k;
        });
        c.stamp(u, v, size * 0.6 + 2.0, |dx, dy, t| {
            let (cc, a) = local(dx, dy);
            let (inside, vein) = leaf_shape(cell, cc, a);
            let cov = smooth(-0.01, 0.01, inside);
            if cov <= 0.0 {
                return;
            }
            let edge = 1.0 - smooth(0.0, 0.05, inside);
            // One side curled up, catching the light.
            let lift = smooth(0.0, 0.25, cc * curl) * edge;
            let shade = (1.0 + 0.15 * vein) * (1.0 - 0.15 * edge) * (1.0 + 0.25 * lift);
            t.color = mix(t.color, col * shade, cov);
            t.height += (0.0008 + 0.0012 * lift) * cov + 0.0002 * vein * cov;
            t.rough = lerp(t.rough, 0.72, cov);
            t.ao = lerp(t.ao, 1.0, cov);
            t.alpha = lerp(t.alpha, 1.0, cov * rng_free(u, v, cell));
        });
    }
    for _ in 0..1200 {
        let (u, v) = (rng.f(), rng.f());
        let col = [hex(0x5e3e26), hex(0x4a3220), hex(0x7a5a3a)][rng.below(3)];
        line(
            &mut c,
            u,
            v,
            rng.range(0.0, TAU),
            rng.range(12.0, 22.0),
            1.2,
            0.3,
            |cov, _, t| {
                t.color = mix(t.color, col, cov * 0.9);
                t.height += 0.0006 * cov;
            },
        );
    }
    for _ in 0..90 {
        let (u, v) = (rng.f(), rng.f());
        let col = hex(0x5a4a3a) * rng.range(0.7, 1.1);
        line(
            &mut c,
            u,
            v,
            rng.range(0.0, TAU),
            rng.range(20.0, 60.0),
            rng.range(2.0, 3.5),
            0.4,
            |cov, _, t| {
                t.color = mix(t.color, col, cov);
                t.height += 0.002 * cov;
            },
        );
    }
    c.bake(1.4, 30.0)
}

/// How solid one leaf stamp is in the coverage (0.6..1): most leaves fully, some only
/// partly, so the litter's edge keeps a few loose leaves rather than all or none.
fn rng_free(u: f32, v: f32, cell: u32) -> f32 {
    0.6 + 0.4
        * unit(super::procgen::hash(
            (u * 4096.0) as i32,
            (v * 4096.0) as i32,
            cell + 1,
        ))
}

// ----------------------------------------------------------------------------- props

/// Weathered softwood: grain along u (2 m) across 0.5 m, latewood ridges, checks and knots,
/// half silvered. Planks pick their own window of it.
fn planks() -> TexSet {
    let mut c = Canvas::new(1024, 256, Vec2::new(2.0, 0.5));
    let (early, late, silver) = (hex(0x8c7c6a), hex(0x4e4337), hex(0x9a9994));
    let mut rng = Rand::new(13);
    let knots: Vec<(Vec2, f32)> = (0..5)
        .map(|_| (Vec2::new(rng.f(), rng.f()), rng.range(0.006, 0.014)))
        .collect();
    c.fill(|u, v| {
        let x = u * 2.0;
        let y = v * 0.5;
        // Rings wander slowly along the board and bend round knots.
        let mut w = fbm2(u, v, 3, 2, 3, 111) * 0.012 + fbm2(u, v, 12, 4, 2, 112) * 0.002;
        let mut knot_core = 0.0f32;
        for &(k, r) in &knots {
            let mut d = Vec2::new(x - k.x * 2.0, (y - k.y * 0.5) * 2.5);
            d.x -= (d.x / 2.0).round() * 2.0;
            d.y -= (d.y / 1.25).round() * 1.25;
            let dist = d.length();
            w += 0.02 * r / (dist + r) * d.y.signum();
            knot_core = knot_core.max(1.0 - smooth(r * 0.5, r * 1.6, dist));
        }
        let rings = (y + w) * 70.0;
        let f = rings - rings.floor();
        let latewood = smooth(0.62, 0.8, f) * (1.0 - smooth(0.9, 1.0, f));
        let fiber = perlin_p(u * 8.0, v * 220.0, 8, 220, 113) * 0.5
            + perlin_p(u * 30.0, v * 600.0, 30, 600, 114) * 0.5;
        let weather = smooth(-0.5, 0.35, fbm2(u, v, 4, 2, 4, 115));
        let mut col = mix(early, late, latewood * 0.8) * (0.92 + 0.12 * fiber);
        col = mix(col, silver * (0.86 + 0.2 * latewood), weather * 0.8);
        col = mix(col, hex(0x3a2a1c), knot_core * 0.85);
        Texel {
            color: col,
            height: latewood * 0.0003 + fiber * 0.0002 + knot_core * 0.0004,
            rough: 0.7 + 0.15 * weather,
            ..Default::default()
        }
    });
    // Drying checks along the grain.
    for _ in 0..40 {
        let (u, v) = (rng.f(), rng.f());
        line(
            &mut c,
            u,
            v,
            rng.normal(0.01),
            rng.range(40.0, 200.0),
            rng.range(1.0, 2.0),
            0.0,
            |cov, along, t| {
                let k = cov * (1.0 - (along - 0.5).abs() * 2.0).sqrt();
                t.height -= 0.0012 * k;
                t.color *= 1.0 - 0.55 * k;
            },
        );
    }
    c.bake(1.5, 40.0)
}

/// Hot-dip galvanized steel: spangles of slightly different sheen, dirt and white rust.
fn galvanized() -> TexSet {
    let mut c = Canvas::new(256, 256, Vec2::splat(0.4));
    c.fill(|u, v| {
        let cell = worley(u * 16.0, v * 16.0, 16, 121, 0.9);
        let dir = Vec2::new(unit(cell.id) - 0.5, unit(cell.id >> 7) - 0.5);
        let sheen = (cell.to.dot(dir) * 1.5).clamp(-0.5, 0.5);
        let dirt = smooth(0.0, 0.7, fbm(u, v, 4, 4, 122));
        // White rust in faint streaks along the length (u), never in round spots.
        let white = smooth(0.45, 0.75, fbm2(u, v, 3, 18, 3, 123));
        let tone = 0.6 + 0.12 * (unit(cell.id >> 3) - 0.5) + 0.04 * sheen;
        let col = Vec3::splat(tone)
            .lerp(Vec3::new(0.25, 0.24, 0.22), dirt * 0.4)
            .lerp(Vec3::splat(0.62), white * 0.2);
        Texel {
            color: col,
            height: -0.00003 * smooth(0.0, 0.05, cell.f2 - cell.f1),
            rough: (0.45 + 0.2 * unit(cell.id >> 11) + dirt * 0.25 + white * 0.15).min(1.0),
            metal: 0.8 - white * 0.3 - dirt * 0.3,
            ..Default::default()
        }
    });
    c.bake(1.0, 5.0)
}

/// The tops of the grating's bearing bars over a 2.56 × 1.92 m patch (u across the
/// walkway from its left edge, v along it; it repeats along only): dull spangled zinc,
/// polished to bright steel down the middle where feet go, rust where the zinc has worn
/// through but feet don't polish it, and grime toward the sides and in patches.
fn tread() -> TexSet {
    let mut c = Canvas::new(512, 384, Vec2::new(2.56, 1.92));
    c.fill(|u, v| {
        let x = u * 2.56 - 1.28;
        // Features `f` per 0.64 m, round in meters (the patch is 4 × 3 of them).
        let n = |f: i32, octaves: u32, seed: u32| fbm2(u, v, f * 4, f * 3, octaves, seed);
        let band = (x - 0.1 * n(1, 2, 141)).abs() + 0.16 * n(3, 3, 142);
        let wear = 1.0 - smooth(0.25, 0.85, band);
        let polish = smooth(0.15, 0.75, wear + 0.25 * n(6, 2, 143));
        let spangle = n(20, 2, 144);
        let zinc = Vec3::new(0.46, 0.46, 0.45) * (1.0 + 0.12 * spangle);
        let bright = Vec3::new(0.84, 0.84, 0.82);
        let grime = ((0.85 * smooth(0.8, 1.22, x.abs()) + 0.6 * smooth(0.15, 0.6, n(4, 3, 145)))
            * (1.0 - 0.8 * polish))
            .min(1.0);
        let rust = smooth(
            0.3,
            0.6,
            n(8, 3, 146) + 0.35 * wear * (1.0 - polish) + 0.25 * smooth(0.9, 1.25, x.abs()),
        );
        let rust_col =
            Vec3::new(0.3, 0.13, 0.06).lerp(Vec3::new(0.5, 0.27, 0.11), 0.5 + 0.5 * n(16, 2, 147));
        let col = zinc
            .lerp(bright, polish)
            .lerp(rust_col, rust * 0.85)
            .lerp(Vec3::new(0.14, 0.12, 0.1), grime * 0.7);
        Texel {
            color: col,
            rough: lerp(0.58 + 0.08 * spangle, 0.45, polish)
                .max(0.85 * rust)
                .max(0.9 * grime),
            metal: lerp(0.8, 1.0, polish) * (1.0 - rust * 0.9) * (1.0 - 0.8 * grime),
            ..Default::default()
        }
    });
    c.bake(1.0, 0.0)
}

/// Four leaves in a 2×2 atlas (maple, oak, beech, chestnut), pale so vertex colours tint
/// them, with veins, blotches and browned edges; alpha cuts the outline.
fn leaf_atlas() -> TexSet {
    let mut c = Canvas::new(512, 512, Vec2::splat(0.2));
    c.fill(|u, v| {
        let cell = (u >= 0.5) as u32 + 2 * (v >= 0.5) as u32;
        let (lx, ly) = ((u * 2.0).fract(), (v * 2.0).fract());
        // Leaf space: a along the midrib (-0.5 stem .. 0.5 tip), c across.
        let (cc, a) = (lx - 0.5, 0.5 - ly);
        let (inside, vein) = leaf_shape(cell, cc, a);
        let blotch = fbm(u, v, 8, 3, 131 + cell);
        let spots = smooth(0.55, 0.7, fbm(u, v, 24, 2, 135));
        let burn = 1.0 - smooth(0.0, 0.06, inside.max(0.0));
        let base = Vec3::new(0.95, 0.9, 0.8) * (0.85 + 0.2 * blotch);
        let col = base * (1.0 + 0.18 * vein) * (1.0 - spots * 0.45);
        let col = col.lerp(Vec3::new(0.45, 0.3, 0.18), burn * 0.6);
        Texel {
            color: col,
            height: vein * 0.0004 + blotch * 0.00015,
            rough: 0.55 + 0.2 * burn,
            alpha: if inside > 0.0 { 1.0 } else { 0.0 },
            ..Default::default()
        }
    });
    c.bake(1.0, 0.0)
}

/// Signed distance-like inside test of the four leaf outlines (> 0 inside, leaf units) and a
/// vein mask (0..1).
fn leaf_shape(cell: u32, c: f32, a: f32) -> (f32, f32) {
    let stem = (0.012 - c.abs()).min(-0.32 - a).min(a + 0.5);
    let midrib = 1.0 - smooth(0.004, 0.012, c.abs());
    match cell {
        0 => {
            // Maple: five pointed lobes round a point near the base.
            let p = Vec2::new(c, a + 0.12);
            let r = p.length();
            let th = p.x.atan2(p.y);
            let lobes = (0.5 + 0.5 * (th * 2.5).cos()).powf(1.6);
            let teeth = 0.02 * (th * 22.0).cos().abs();
            let outline = 0.18 + 0.27 * lobes + teeth;
            let notch = smooth(2.4, 3.1, th.abs());
            let inside = (outline * (1.0 - 0.6 * notch) - r).max(stem);
            let veins = (0..5)
                .map(|k| {
                    let ang = (k as f32 - 2.0) * TAU / 5.0;
                    let d = Vec2::new(ang.sin(), ang.cos());
                    let along = p.dot(d);
                    let off = (p - d * along).length();
                    (1.0 - smooth(0.004, 0.012, off)) * (along > 0.0) as u8 as f32
                })
                .fold(0.0f32, f32::max);
            (inside, veins.max(midrib * (a > -0.12) as u8 as f32))
        }
        1 => {
            // Oak: long with rounded lobes.
            let t = a + 0.5;
            let env = (std::f32::consts::PI * t.clamp(0.0, 1.0)).sin().powf(0.7) * 0.27;
            let lobes = 0.7 + 0.3 * (t * TAU * 3.5).sin().abs();
            let inside = (env * lobes - c.abs()).min(0.44 - a).min(a + 0.36);
            // Side veins run from the midrib out into each lobe.
            let q = ((a - c.abs() * 0.6) * 7.0).fract();
            let side = 1.0 - smooth(0.004, 0.012, q.min(1.0 - q) * 0.14);
            (
                inside.max(stem),
                midrib.max(side * 0.7 * (inside > 0.015) as u8 as f32),
            )
        }
        2 => {
            // Beech: an oval with fine teeth and straight parallel veins.
            let t = a + 0.5;
            let env =
                (std::f32::consts::PI * t.clamp(0.0, 1.0)).sin().powf(0.8) * (0.31 - 0.08 * t);
            let teeth = 0.01 * (t * 60.0).sin().abs();
            let inside = (env - teeth - c.abs()).min(0.46 - a).min(a + 0.38);
            let veins = 1.0
                - smooth(
                    0.006,
                    0.016,
                    ((a - c.abs() * 0.9) * 11.0)
                        .fract()
                        .min(1.0 - ((a - c.abs() * 0.9) * 11.0).fract())
                        * 0.09,
                );
            (
                inside.max(stem),
                midrib.max(veins * 0.6 * (inside > 0.02) as u8 as f32),
            )
        }
        _ => {
            // Chestnut: long, narrow and saw-toothed.
            let t = a + 0.5;
            let env = (std::f32::consts::PI * t.clamp(0.0, 1.0)).sin().powf(0.9) * 0.16;
            let teeth = 0.018 * (t * 34.0).fract();
            let inside = (env - teeth - c.abs()).min(0.48 - a).min(a + 0.42);
            let veins = 1.0
                - smooth(
                    0.004,
                    0.012,
                    ((a - c.abs() * 1.2) * 16.0)
                        .fract()
                        .min(1.0 - ((a - c.abs() * 1.2) * 16.0).fract())
                        * 0.06,
                );
            (
                inside.max(stem),
                midrib.max(veins * 0.5 * (inside > 0.01) as u8 as f32),
            )
        }
    }
}

/// Granite-like stone, pale so vertex colours tint each rock.
fn stone() -> TexSet {
    let mut c = Canvas::new(256, 256, Vec2::splat(0.3));
    c.fill(|u, v| {
        let n = fbm(u, v, 4, 5, 141);
        let px = super::procgen::hash((u * 256.0) as i32, (v * 256.0) as i32, 142);
        let crystals = worley(u * 60.0, v * 60.0, 60, 143, 1.0);
        let kind = unit(crystals.id);
        let mut col = Vec3::splat(0.78 + 0.12 * n);
        if kind > 0.82 {
            col *= 0.35;
        } else if kind > 0.7 {
            col *= Vec3::new(1.08, 0.98, 0.94);
        }
        col *= 0.94 + 0.12 * unit(px);
        Texel {
            color: col,
            height: n * 0.0015 + (unit(px) - 0.5) * 0.0002,
            rough: 0.62 + 0.2 * (kind > 0.82) as u8 as f32,
            ..Default::default()
        }
    });
    c.bake(1.4, 20.0)
}
