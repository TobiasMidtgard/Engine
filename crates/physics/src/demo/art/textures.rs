//! Procedural PBR texture sets for the physics proving ground: concrete, wood, steel, rubber,
//! vinyl, safety paint and grass, generated in code at startup (no files to ship).
//!
//! Every set is
//! * **tileable**: all noise is periodic over the tile, scattered features wrap around it;
//! * **deterministic**: fixed seeds, the same pixels on every run and machine;
//! * **physically scaled**: features are sized in meters and heights are in meters; normals and
//!   ambient occlusion are derived from that height field, so `normal_scale = 1` is correct.
//!
//! Meshes use metric UVs (1 UV unit = 1 m), so a material uses `uv_scale = 1 / tile_m`:
//!
//! ```ignore
//! let set = textures::concrete_slab(1024);
//! let s = 1.0 / set.tile_m;
//! let orm = images.add(set.orm);
//! let mat = StandardMaterial::hex(0xffffff)    // or a tint for rubber, vinyl, crates
//!     .with_base_color_texture(images.add(set.albedo))
//!     .with_normal_texture(images.add(set.normal))
//!     .with_metallic_roughness_texture(orm.clone()) // the same ORM image serves both slots
//!     .with_occlusion_texture(orm)
//!     .with_roughness(1.0)                     // the texture holds the real values:
//!     .with_metallic(1.0)                      // factors of 1 pass them through
//!     .with_uv_scale(s, s);
//! ```
//!
//! Channel conventions are those of `engine_render::StandardMaterial` (glTF): albedo is sRGB,
//! the normal map is linear tangent space with +Y = up in the image (OpenGL), and the ORM map
//! is linear with R = occlusion, G = roughness, B = metallic. Image rows run top to bottom
//! (+V), so "down" on a wall whose V grows downward is +V (rust and water streaks run that
//! way).
//!
//! Generation splits rows over all cores (`std::thread::scope`).

use engine_render::Image;
use std::f32::consts::{FRAC_1_SQRT_2, TAU};
use std::sync::Mutex;

/// A tileable PBR texture set. Meshes use metric UVs (1 UV unit = 1 m), so a material uses
/// `uv_scale = 1 / tile_m`.
#[derive(Clone, Debug)]
pub struct TextureSet {
    /// sRGB RGBA8 albedo (alpha 255).
    pub albedo: Image,
    /// Linear tangent-space normal map, +Y up (glTF / OpenGL).
    pub normal: Image,
    /// Linear: R = ambient occlusion, G = roughness, B = metallic (glTF ORM).
    pub orm: Image,
    /// Meters one tile covers.
    pub tile_m: f32,
}

// =============================================================================================
// Material sets
// =============================================================================================

/// Power-troweled concrete for the 70 × 50 m yard floor. `tile_m = 6`: saw-cut control joints
/// run along the tile edges, so the floor shows a 6 m joint grid. Sand-grain and pebbly relief,
/// cloudy darker and glossier burnished patches, swirl trowel marks, exposed aggregate where
/// traffic wore the paste, pores, oil stains and drips, water tide marks, tire scuffs and
/// hairline cracks. Roughness 0.65 (oil, burnished) .. 0.97 (joints), mostly 0.75 - 0.9.
///
/// `res` is the texture size in pixels (power of two; 1024 recommended: 5.9 mm per texel).
pub fn concrete_slab(res: u32) -> TextureSet {
    const T: f32 = 6.0;
    let n = res.max(16);
    let texel = T / n as f32;
    let low = Field::<11>::new(n / 4, |u, v| {
        let wu = fbm(u, v, 6, 6, 4, 0.5, 101);
        let wv = fbm(u, v, 6, 6, 4, 0.5, 102);
        [
            fbm(u, v, 3, 3, 4, 0.5, 103),
            fbm(u, v, 2, 2, 3, 0.5, 104),
            wu,
            wv,
            fbm(u + 0.05 * wu, v + 0.05 * wv, 4, 4, 5, 0.55, 105),
            fbm(u, v, 2, 2, 3, 0.5, 106),
            fbm(u, v, 5, 5, 3, 0.5, 107),
            fbm(u + 0.03 * wv, v + 0.03 * wu, 8, 8, 4, 0.55, 108),
            fbm(u, v, 12, 12, 3, 0.5, 109),
            fbm(u, v, 4, 4, 4, 0.5, 110),
            fbm(u, v, 24, 24, 3, 0.55, 113),
        ]
    });
    let mut rng = Rng(7);
    let mut oil = spots(&mut rng, 3, T, (0.2, 0.45), (0.2, 0.38));
    oil.extend(spots(&mut rng, 14, T, (0.01, 0.035), (0.2, 0.45)));
    let oil = Strokes::new(12, oil);
    let tires = Strokes::new(12, tire_marks(&mut Rng(17), T, 6));
    let trowel = Strokes::new(
        16,
        swirl_marks(&mut Rng(27), T, 160, (0.25, 0.7), (0.05, 0.12)),
    );
    let sand_f = (n / 3).max(8) as i32;

    let mut cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 111);
        let sand = perlin(u * sand_f as f32, v * sand_f as f32, sand_f, sand_f, 112);
        let mid = l[10];
        let bump = perlin(u * 150.0, v * 150.0, 150, 150, 123);
        // Cement paste: warm mid grey, cloudy finishing patches, sand-scale grain. The
        // power trowel burnished some areas darker and glossier.
        let burnish = sstep(-0.05, 0.35, l[7] + 0.3 * l[0]);
        let lum = 0.3
            * (1.0 + 0.08 * l[0] + 0.12 * l[7] + 0.07 * mid + 0.06 * sand + 0.07 * grain
                - 0.1 * burnish);
        let mut c = [lum * (1.02 + 0.03 * l[1]), lum, lum * (0.95 - 0.04 * l[1])];
        let mut r = 0.87 + 0.03 * l[7] + 0.04 * mid + 0.03 * grain - 0.17 * burnish;
        let mut h = 0.0004 * mid + 0.0004 * bump + 0.0004 * sand + 0.0001 * grain;
        let mut ao = 1.0;

        // Power-trowel swirls: burnished, slightly darker and glossier arcs.
        let mut swirl = 0.0f32;
        trowel.visit(u, v, |s, d, t| {
            let across = 1.0 - (d / s.hw).abs();
            let k = s.k * sstep(0.0, 0.7, across) * sstep(0.0, 0.3, t) * sstep(1.0, 0.7, t);
            swirl = swirl.max(k);
        });
        c = scale3(c, 1.0 - 0.06 * swirl);
        r -= 0.05 * swirl;
        h -= 0.0001 * swirl;

        // Exposed aggregate, densest where traffic wore the paste away; then air-void pores.
        let expo = sstep(-0.15, 0.35, l[9]);
        for (cells, rmin, rmax, p, seed) in [
            (200, 0.0025, 0.008, 0.02 + 0.3 * expo, 114u32),
            (450, 0.0012, 0.0035, 0.04 + 0.3 * expo, 115),
        ] {
            let w = worley(u, v, cells, 0.9, seed);
            if unit(hash(w.id ^ 0x51)) < p {
                let rad = lerp(rmin, rmax, unit(hash(w.id ^ 0x52))) * (1.0 + 0.35 * sand);
                let d = w.d * T / cells as f32;
                let cov = sat((rad - d) / texel + 0.5);
                if cov > 0.0 {
                    let pick = unit(hash(w.id ^ 0x53));
                    let tone = 0.85 + 0.3 * unit(hash(w.id ^ 0x54));
                    let stone = if pick < 0.5 {
                        scale3(c, 1.15)
                    } else if pick < 0.78 {
                        [0.19, 0.19, 0.18]
                    } else if pick < 0.93 {
                        [0.36, 0.31, 0.24]
                    } else {
                        [0.5, 0.49, 0.47]
                    };
                    c = mix3(c, scale3(stone, tone), cov * 0.8);
                    r = lerp(r, 0.68, cov * 0.8);
                    h += cov * 0.0004 * sat(1.0 - d / rad).sqrt();
                }
            }
        }
        let pore = worley(u, v, 300, 0.9, 116);
        if unit(hash(pore.id ^ 0x61)) < 0.08 + 0.12 * expo {
            let rad = lerp(0.0008, 0.0028, unit(hash(pore.id ^ 0x62)));
            let cov = sat((rad - pore.d * T / 300.0) / texel + 0.5);
            c = scale3(c, 1.0 - 0.55 * cov);
            h -= 0.0012 * cov;
            ao *= 1.0 - 0.5 * cov;
            r = lerp(r, 0.95, cov);
        }

        // Water: whitish efflorescence tide lines around damp, slightly darker patches.
        let region = sstep(0.05, 0.3, l[5]);
        let ring = (-((l[4] - 0.16) / 0.018).powi(2)).exp() * region;
        let damp = sstep(0.16, 0.26, l[4]) * region;
        c = mix3(scale3(c, 1.0 - 0.07 * damp), [0.48, 0.47, 0.45], 0.3 * ring);
        r += 0.03 * ring;

        // Oil: big faint stains and small drips, darker rim, glossier.
        let mut oil_k = 0.0f32;
        oil.visit(u, v, |s, d, _| {
            let q = d.abs() / (s.hw / 1.5 * (1.0 + 0.5 * l[8]));
            let core = 1.0 - sstep(0.4, 1.0, q);
            let rim = (-((q - 0.9) / 0.1).powi(2)).exp();
            oil_k = oil_k.max(s.k * (0.75 * core + 0.35 * rim));
        });
        let oil_k = sat(oil_k);
        c = mul3(
            c,
            [1.0 - 0.5 * oil_k, 1.0 - 0.53 * oil_k, 1.0 - 0.58 * oil_k],
        );
        r = lerp(r, 0.6, 0.75 * oil_k);

        // Tire scuffs: rubber smeared in streaks along the arc.
        let mut tire = 0.0f32;
        tires.visit(u, v, |s, d, t| {
            let x = d / s.hw;
            let tread = 0.55 + 0.45 * perlin(x * 7.0 + s.id as f32 * 3.1, t * 6.0, 1 << 16, 6, 117);
            let k = s.k * sstep(1.0, 0.55, x.abs()) * sstep(0.0, 0.2, t) * sstep(1.0, 0.75, t);
            tire = tire.max(k * tread);
        });
        c = scale3(c, 1.0 - 0.4 * tire);
        r = lerp(r, 0.72, 0.5 * tire);

        // Hairline cracks: parts of a warped Voronoi network, with a fine jagged wander.
        let mut crack = 0.0f32;
        let mut jag = None;
        for (cells, lo, seed) in [(3, 0.08, 119u32), (7, 0.32, 120)] {
            // Skip the network where it cannot be visible (the per-cell term is >= -0.125).
            if l[6] < lo - 0.13 {
                continue;
            }
            let j = *jag.get_or_insert_with(|| fbm(u, v, 48, 48, 2, 0.5, 118));
            let (wu, wv) = (u + 0.035 * l[2] + 0.0015 * j, v + 0.035 * l[3] - 0.0015 * j);
            let (e, id) = voronoi_edge(wu, wv, cells, 0.8, seed);
            let vis = sstep(lo, lo + 0.22, l[6] + 0.25 * (unit(hash(id)) - 0.5));
            if vis > 0.0 {
                let e_m = e * T / cells as f32;
                crack = crack.max(vis * (-(e_m / (0.55 * texel)).powi(2)).exp());
            }
        }
        c = scale3(c, 1.0 - 0.6 * crack);
        h -= 0.0015 * crack;
        ao *= 1.0 - 0.35 * crack;

        // Saw-cut control joints on the tile edges, chipped arrises and dirt along them.
        let mut joint = 0.0f32;
        let mut near = 0.0f32;
        for (d_axis, along, seed) in [(u.min(1.0 - u), v, 121u32), (v.min(1.0 - v), u, 122)] {
            let d = d_axis * T;
            if d < 0.09 {
                let chip = sstep(0.25, 0.7, fbm(along, 0.37, 140, 1, 3, 0.5, seed));
                let hw = 0.0042 + 0.007 * chip;
                joint = joint.max(sat((hw - d) / texel + 0.5));
                near = near.max(sat(1.0 - (d - hw).max(0.0) / 0.08).powi(2));
                h -= 0.0012 * sstep(hw + 0.006, hw, d);
            }
        }
        let dirt = 0.6 + 0.4 * sstep(-0.3, 0.4, mid);
        c = scale3(c, 1.0 - 0.14 * near * dirt);
        c = mix3(c, [0.035, 0.033, 0.03], joint);
        h -= 0.016 * joint;
        ao *= 1.0 - 0.75 * joint;
        r = lerp(r, 0.97, joint);

        Px {
            h,
            c,
            r: r.clamp(0.6, 0.97),
            ao,
            ..Px::default()
        }
    });
    cv.normal_gain = 2.0;
    cv.finish(&[(2, 0.0005, 0.45), (8, 0.0015, 0.35)])
}

/// Formwork-cast concrete for walls, stairs and posts. `tile_m = 3`: horizontal board-form
/// lines (boards ~19 cm with small steps between them, lines along U, a butt joint per board),
/// board grain imprint, clustered bug holes, a 1.5 m grid of plugged form-tie holes with rust
/// runs below them, dirt streaks running down (+V) and soft mottling. Roughness ~0.65 - 0.9.
pub fn cast_concrete(res: u32) -> TextureSet {
    const T: f32 = 3.0;
    const BOARDS: usize = 16;
    let n = res.max(16);
    let texel = T / n as f32;
    // Board seams (in V), with jitter; the first seam sits on the tile edge so it wraps.
    let mut rng = Rng(31);
    let mut seams = [0.0f32; BOARDS + 1];
    for (i, s) in seams.iter_mut().enumerate().take(BOARDS).skip(1) {
        *s = (i as f32 + rng.range(-0.18, 0.18)) / BOARDS as f32;
    }
    seams[BOARDS] = 1.0;
    let boards: Vec<(f32, f32, f32)> = (0..BOARDS)
        .map(|_| (rng.next(), rng.range(-0.0004, 0.0004), rng.range(-1.0, 1.0)))
        .collect();
    let ties: Vec<(f32, f32, f32)> = (0..4)
        .map(|i| {
            let (gx, gy) = ((i % 2) as f32, (i / 2) as f32);
            (0.25 + 0.5 * gx, 0.27 + 0.5 * gy, rng.range(0.3, 1.0))
        })
        .collect();
    let low = Field::<7>::new(n / 2, |u, v| {
        [
            fbm(u, v, 3, 3, 4, 0.5, 201),
            fbm(u, v, 10, 10, 4, 0.5, 202),
            fbm(u, v, 48, 2, 3, 0.5, 203),
            fbm(u, v, 3, 2, 3, 0.5, 204),
            fbm(u, v, 2, 2, 3, 0.5, 205),
            fbm(u, v, 5, 5, 3, 0.5, 206),
            fbm(u, v, 24, 1, 3, 0.5, 207),
        ]
    });
    let sand_f = (n / 3).max(8) as i32;

    let mut cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 208);
        let sand = perlin(u * sand_f as f32, v * sand_f as f32, sand_f, sand_f, 209);

        // Which board, and how far from its seams and its butt joint.
        let b = seams.partition_point(|&s| s <= v).clamp(1, BOARDS) - 1;
        let (s0, s1) = (seams[b], seams[b + 1]);
        let seam_d = (v - s0).min(s1 - v) * T;
        let (butt, offset, tone) = boards[b];
        let wood = fbm(
            u,
            (v - s0) * 0.35 + b as f32 * 0.37,
            6,
            80,
            3,
            0.55,
            210 + b as u32,
        );
        let butt_d = wrap_d(u, butt).abs() * T;

        let blotch = sstep(0.05, 0.4, l[5]);
        let lum = 0.33
            * (1.0 + 0.12 * l[0] + 0.1 * l[1] + 0.1 * tone + 0.06 * sand + 0.05 * grain
                - 0.1 * blotch);
        let mut c = [
            lum * (1.0 + 0.02 * l[4]),
            lum * 0.99,
            lum * (0.965 - 0.02 * l[4]),
        ];
        let mut r = 0.8 + 0.05 * l[1] + 0.03 * grain + 0.04 * tone - 0.05 * blotch;
        let mut h = offset + 0.0004 * l[1] + 0.0003 * sand + 0.00008 * grain + 0.0003 * wood;
        let mut ao = 1.0;
        // Board imprint: grain and a thin fin where cement bled into the seam.
        c = scale3(c, 1.0 + 0.09 * wood);
        let fin = (-(seam_d / (0.7 * texel)).powi(2)).exp();
        let fin = fin.max((-(butt_d / (0.7 * texel)).powi(2)).exp() * 0.7);
        h += 0.0004 * fin;
        c = scale3(c, 1.0 - 0.1 * fin);
        r -= 0.04 * fin;

        // Dirt streaks running down the wall, darker and smoother.
        let streak = sstep(0.0, 0.3, l[2]) * sstep(-0.1, 0.35, l[3]);
        c = mul3(
            c,
            [
                1.0 - 0.24 * streak,
                1.0 - 0.23 * streak,
                1.0 - 0.22 * streak,
            ],
        );
        r -= 0.1 * streak;
        let lime = sstep(0.25, 0.45, l[6]) * sstep(0.1, 0.4, l[3]) * 0.5;
        c = mix3(c, [0.5, 0.49, 0.47], 0.25 * lime);

        // Bug holes (air bubbles against the form), in clusters.
        let density = 0.3 + 1.2 * sstep(-0.25, 0.35, l[5]);
        for (cells, rmin, rmax, p, seed) in [
            (70, 0.0015, 0.004, 0.25, 211u32),
            (24, 0.004, 0.009, 0.1, 212),
        ] {
            let w = worley(u, v, cells, 0.85, seed);
            if unit(hash(w.id ^ 0x71)) < p * density {
                let rad = lerp(rmin, rmax, unit(hash(w.id ^ 0x72))) * (1.0 + 0.3 * sand);
                let d = w.d * T / cells as f32;
                let cov = sat((rad - d) / texel + 0.5);
                c = scale3(c, 1.0 - 0.6 * cov);
                h -= 0.003 * cov * sat(1.0 - d / rad).sqrt();
                ao *= 1.0 - 0.6 * cov;
                r = lerp(r, 0.92, cov);
            }
        }

        // Form-tie holes: recessed cone, darker grout plug, rust run below some of them.
        for &(tx, ty, rust) in &ties {
            let (dx, dy) = (wrap_d(u, tx) * T, wrap_d(v, ty) * T);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 0.05 {
                let hole = sat((0.013 - d) / texel + 0.5);
                let plug = sat((0.022 - d) / texel + 0.5);
                c = mix3(c, scale3(c, 0.8), plug);
                c = mix3(c, [0.09, 0.085, 0.08], hole * 0.85);
                h -= 0.0015 * plug + 0.01 * hole * sat(1.0 - d / 0.013);
                ao *= 1.0 - 0.6 * hole;
                r = lerp(r, 0.9, plug);
            }
            if dy > 0.0 && dy < 0.6 && dx.abs() < 0.04 {
                let fall = (1.0 - dy / 0.6).powi(2) * sstep(0.0, 0.02, dy);
                let w = 0.006 + 0.025 * dy / 0.6;
                let run = rust * fall * sstep(w, w * 0.3, dx.abs()) * (0.6 + 0.4 * l[2].abs());
                c = mix3(c, [0.22, 0.11, 0.05], 0.55 * run);
            }
        }
        Px {
            h,
            c,
            r: r.clamp(0.6, 0.95),
            ao,
            ..Px::default()
        }
    });
    cv.normal_gain = 2.0;
    cv.finish(&[(2, 0.0005, 0.4), (6, 0.0012, 0.3)])
}

/// Pine/spruce boards for shipping crates. `tile_m = 1`, grain along U. Flat-sawn growth rings
/// with cathedral figures, knots with swirling grain, bandsaw marks, fibers and resin canals,
/// nail holes with iron stains, light dirt. Albedo averages about sRGB `#cfaf80` and stays fairly
/// neutral so `base_color` can tint crate variants. Roughness ~0.55 (knots) .. 0.85.
pub fn crate_wood(res: u32) -> TextureSet {
    crate_wood_seeded(res, 0)
}

/// [`crate_wood`] with another seed: same look, different boards (for crate variants that
/// should not show identical faces).
pub fn crate_wood_seeded(res: u32, seed: u32) -> TextureSet {
    const T: f32 = 1.0;
    const RINGS: f32 = 34.0;
    let n = res.max(16);
    let texel = T / n as f32;
    let sd = (seed % 4_000_000) * 1000;
    let mut rng = Rng(41 ^ seed.wrapping_mul(0x9e37));
    let figures = cathedral_figures(&mut rng, 4, (2.5, 4.5), (0.025, 0.05));
    let knots: Vec<Knot> = (0..2 + (rng.next() * 1.6) as usize)
        .map(|_| Knot {
            x: rng.next(),
            y: rng.next(),
            rx: rng.range(0.014, 0.026),
            ry: rng.range(0.011, 0.018),
            push: rng.range(3.0, 5.0),
            dead: rng.next() < 0.4,
            crack: rng.range(0.0, TAU),
        })
        .collect();
    let nails: Vec<(f32, f32)> = (0..3)
        .flat_map(|_| {
            let (x, y) = (rng.next(), rng.next());
            [
                (x, y),
                (x + rng.range(-0.004, 0.004), y + rng.range(0.03, 0.05)),
            ]
        })
        .collect();
    let low = Field::<6>::new(n / 2, |u, v| {
        [
            fbm(u, v, 1, 3, 3, 0.5, sd + 401),
            fbm(u, v, 2, 6, 2, 0.5, sd + 402),
            fbm(u, v, 2, 4, 3, 0.5, sd + 403),
            fbm(u, v, 4, 4, 4, 0.5, sd + 404),
            fbm(u, v, 3, 4, 2, 0.5, sd + 405),
            fbm(u, v, 3, 3, 3, 0.5, sd + 406),
        ]
    });
    let oct_fiber = octaves_below(64, 3, n);

    let mut cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, sd + 407);
        let mut phi = v * RINGS + 1.6 * l[0] + 0.5 * l[1] + figure_push(&figures, u, v, T);
        let mut knot_px: Option<(f32, f32, &Knot)> = None;
        let mut halo = 0.0f32;
        for k in &knots {
            let (dx, dy) = (wrap_d(u, k.x) * T, wrap_d(v, k.y) * T);
            let e = (dx / (k.rx * 3.2)).powi(2) + (dy / (k.ry * 2.2)).powi(2);
            if e < 9.0 {
                phi += k.push * (-e).exp();
                halo = halo.max((-e * 0.7).exp());
                let q = ((dx / k.rx).powi(2) + (dy / k.ry).powi(2)).sqrt();
                if q < 1.0 + 2.0 * texel / k.ry {
                    knot_px = Some((q, dy.atan2(dx), k));
                }
            }
        }
        // Uneven ring widths: warp the phase by a function of the phase itself.
        phi += 0.45 * perlin(phi / RINGS * 7.0, 0.5, 7, 1, sd + 408);
        let ring = phi.floor();
        let t = phi - ring;
        let hr = unit(hash2((ring as i32).rem_euclid(RINGS as i32), 1, sd + 409));
        let late =
            sstep(0.35 + 0.25 * hr, 0.86, t) * (1.0 - sstep(0.9, 0.99, t)) * (0.5 + 0.5 * hr);
        let fiber = fbm(u, v, 4, 64, oct_fiber, 0.6, sd + 410);
        let canal = sstep(0.42, 0.62, fbm(u, v, 30, 160, 1, 0.5, sd + 411));

        let early_c = [0.7, 0.49, 0.25];
        let late_c = [0.43, 0.25, 0.095];
        let mut c = mix3(early_c, late_c, late);
        c = scale3(
            c,
            1.0 + 0.06 * l[2] + 0.08 * fiber - 0.08 * canal + 0.03 * grain,
        );
        c = mul3(c, [1.0 + 0.03 * l[2], 1.0, 1.0 - 0.05 * l[2]]);
        c = mul3(c, [1.0 - 0.1 * halo, 1.0 - 0.15 * halo, 1.0 - 0.22 * halo]);
        // Boards stored outdoors grey a little in patches.
        let grey = sstep(0.1, 0.5, l[5]) * 0.25;
        let lum = 0.3 * c[0] + 0.6 * c[1] + 0.1 * c[2];
        c = mix3(c, [lum * 1.02, lum, lum * 0.95], grey);
        let mut r = 0.8 - 0.08 * late + 0.05 * fiber + 0.03 * grain;
        let mut h = 0.00012 * late + 0.00008 * fiber - 0.00003 * canal + 0.00002 * grain;
        let mut ao = 1.0;

        // Bandsaw marks across the grain, in patches.
        let saw_phase = u * 150.0 + v * 6.0 + 0.25 * l[4];
        let saw = (0.5 + 0.5 * (TAU * saw_phase).sin()).powi(3) * sstep(-0.15, 0.35, l[4]);
        h += 0.00012 * saw;
        c = scale3(c, 1.0 + 0.04 * saw);
        r += 0.03 * saw;

        if let Some((q, ang, k)) = knot_px {
            let pw = texel / k.ry;
            let cov = sstep(1.0 + pw, 1.0 - pw, q);
            let wob = 0.12 * (ang * 3.0).sin() + 0.06 * (ang * 7.0 + 1.0).sin();
            let rings = 0.5 + 0.5 * (TAU * (q * 5.0 + wob)).cos();
            let mut kc = mix3(
                [0.33, 0.17, 0.07],
                [0.2, 0.09, 0.035],
                rings * 0.5 + 0.5 * q,
            );
            let rim = (-((q - 0.93) / 0.08).powi(2)).exp();
            kc = mix3(
                kc,
                [0.07, 0.04, 0.02],
                rim * if k.dead { 0.9 } else { 0.45 },
            );
            // Radial drying check.
            let off = (ang - k.crack).sin().abs() * q * k.ry;
            let check = if (ang - k.crack).cos() > 0.0 && q > 0.25 {
                (-(off / (0.5 * texel)).powi(2)).exp()
            } else {
                0.0
            };
            kc = scale3(kc, 1.0 - 0.7 * check);
            c = mix3(c, kc, cov);
            r = lerp(r, 0.55, cov);
            h += cov * (0.0002 * (1.0 - q * q).max(0.0) - 0.0005 * check);
            ao *= 1.0 - 0.4 * check * cov;
        }

        // Nail holes with blue-black iron-tannin stains.
        for &(nx, ny) in &nails {
            let (dx, dy) = (wrap_d(u, nx) * T, wrap_d(v, ny) * T);
            let d = (dx * dx + dy * dy).sqrt();
            if d < 0.012 {
                let hole = sat((0.0014 - d) / texel + 0.5);
                let stain = (-(d / 0.0045).powi(2)).exp();
                c = mix3(c, [0.12, 0.11, 0.1], 0.5 * stain);
                c = mix3(c, [0.03, 0.025, 0.02], hole);
                h -= 0.002 * hole + 0.0002 * stain;
                ao *= 1.0 - 0.7 * hole;
            }
        }

        // Light dirt and handling grime.
        let dirt = sstep(0.05, 0.55, l[3]);
        c = mix3(c, mul3(c, [0.8, 0.78, 0.76]), dirt * 0.6);
        r += 0.03 * dirt;
        Px {
            h,
            c,
            r: r.clamp(0.5, 0.92),
            ao,
            ..Px::default()
        }
    });
    cv.normal_gain = 2.5;
    let cav = cv.cavity(2);
    cv.modify(|p, i| {
        // Dust settles into the low spots.
        let k = sat(cav[i] / 0.00008) * 0.3;
        p.c = mix3(p.c, mul3(p.c, [0.8, 0.77, 0.74]), k);
    });
    cv.finish(&[(2, 0.00015, 0.4), (6, 0.0004, 0.25)])
}

/// Grey-brown aged oak for bridge planks, the seesaw and the paddle wheel. `tile_m = 2`, grain
/// along U. UV-silvered surface with brown left in the eroded earlywood, raised fibrous
/// latewood ridges, torn fibers, ray flecks, deep drying checks with eroded dark wet edges,
/// and dark weather streaks. Roughness ~0.45 (wet crack edges) .. 0.95.
pub fn weathered_timber(res: u32) -> TextureSet {
    const T: f32 = 2.0;
    const RINGS: f32 = 64.0;
    let n = res.max(16);
    let texel = T / n as f32;
    let mut rng = Rng(61);
    // Rift-sawn planks: only gentle waves, no pointed cathedrals.
    let mut figures = cathedral_figures(&mut rng, 3, (1.0, 2.0), (0.05, 0.08));
    for f in &mut figures {
        f.rise *= 3.0;
        f.fade *= 1.2;
    }
    let low = Field::<7>::new(n / 2, |u, v| {
        [
            fbm(u, v, 1, 4, 3, 0.5, 601),
            fbm(u, v, 2, 8, 2, 0.5, 602),
            fbm(u, v, 3, 3, 4, 0.5, 603),
            fbm(u, v, 3, 20, 3, 0.5, 604),
            fbm(u, v, 6, 6, 4, 0.5, 605),
            fbm(u, v, 2, 24, 3, 0.5, 606),
            fbm(u, v, 5, 40, 2, 0.5, 607),
        ]
    });
    let oct_fiber = octaves_below(70, 3, n);
    let rows = n as i32;

    let cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 608);
        let mut phi = v * RINGS + 1.6 * l[0] + 0.6 * l[1] + figure_push(&figures, u, v, T);
        phi += 0.35 * perlin(phi / RINGS * 9.0, 0.5, 9, 1, 609);
        let ring = phi.floor();
        let t = phi - ring;
        let ring_id = (ring as i32).rem_euclid(RINGS as i32);
        let hr = unit(hash2(ring_id, 7, 610));
        let late = sstep(0.3 + 0.2 * hr, 0.75, t) * (1.0 - sstep(0.85, 0.99, t));
        // Fibers run along the board (U) whatever the ring figure does.
        let fiber = fbm(u, v, 3, 70, oct_fiber, 0.65, 611);
        // Torn fibers: one-texel streaks along the grain, 4 - 8 cm long.
        let row = ((v * rows as f32).floor() as i32).rem_euclid(rows);
        let seg = (u * 32.0 + 9.0 * unit(hash2(row, 1, 612))).floor() as i32;
        let torn = unit(hash2(row, seg.rem_euclid(32), 613)) - 0.5;
        let flecks = sstep(0.42, 0.68, fbm(u, v, 24, 100, 2, 0.5, 614));

        // Drying checks follow a ring: a check on ring k exists where its own 1D noise says
        // so. Their edges are eroded round (a dip either side) and stay damp and dark.
        let chosen = unit(hash2(ring_id, 3, 615)) < 0.2;
        let mut crack = 0.0f32;
        let mut wet = 0.0f32;
        let mut lip = 0.0f32;
        if chosen {
            let open = fbm(
                u,
                (ring_id as f32 + 0.5) / RINGS,
                3,
                RINGS as i32,
                3,
                0.5,
                616,
            );
            let open = sstep(0.0, 0.3, open + 0.15 * l[2]);
            if open > 0.0 {
                let ragged = 1.0 + 0.45 * perlin(u * 90.0, ring_id as f32, 90, RINGS as i32, 617);
                let hw = (0.03 + 0.22 * open) * ragged * T / RINGS;
                let d = (t - 0.5).abs() * T / RINGS;
                crack = sat((hw - d) / texel + 0.5);
                let out = (d - hw).max(0.0);
                lip = (-(out / 0.004).powi(2)).exp() * open;
                wet = (-(out / 0.008).powi(2)).exp() * sstep(0.0, 0.15, open);
            }
        }

        // Silvering: grey on the exposed ridges, brown down in the eroded earlywood; some
        // areas weathered less (warmer).
        let ridge = sat(0.25 + 0.75 * late * (0.6 + 0.4 * hr) + 0.35 * fiber + 0.3 * torn);
        let weather = sat(0.8 + 0.4 * l[2] + 0.2 * l[4]);
        let silver = sat(ridge * weather);
        let groove = mix3([0.13, 0.105, 0.08], [0.1, 0.08, 0.06], sat(0.5 + fiber));
        let grey = scale3(
            [0.32, 0.31, 0.29],
            1.0 + 0.2 * fiber + 0.12 * torn + 0.06 * l[4] + 0.06 * grain,
        );
        let mut c = mix3(groove, grey, silver);
        c = scale3(c, 1.0 - 0.22 * flecks);
        // Dark weather streaks along the grain.
        let streak = sstep(0.15, 0.5, l[3]) * sstep(0.0, 0.5, l[5] + 0.3);
        c = mix3(c, [0.07, 0.066, 0.062], 0.55 * streak);
        let mut r = 0.9 - 0.04 * late + 0.04 * fiber;

        let mut h = 0.0009 * late * (0.6 + 0.4 * hr) + 0.00035 * fiber + 0.00012 * torn
            - 0.00015 * flecks
            + 0.0004 * l[6]
            + 0.00005 * grain;
        let mut ao = 1.0;

        // Wet, dark edges and a deep dark crack.
        c = scale3(c, 1.0 - 0.5 * wet);
        r = lerp(r, 0.45, wet);
        c = mix3(c, [0.03, 0.022, 0.016], crack);
        r = lerp(r, 0.5, crack);
        h -= 0.009 * crack + 0.0012 * lip;
        ao *= 1.0 - 0.75 * crack;
        Px {
            h,
            c,
            r: r.clamp(0.4, 0.96),
            ao,
            ..Px::default()
        }
    });
    cv.finish(&[(2, 0.0005, 0.45), (6, 0.0015, 0.35)])
}

/// Industrial painted steel. `tile_m = 1.5`. `paint` is the linear RGB paint color (use a white
/// `base_color`), `wear` 0 (nearly pristine) .. 1 (heavily chipped). Orange peel, chips with a
/// visible paint edge through grey primer to bare steel (metallic only there), rust in old
/// chips with rust runs below (+V), scratches, chalky faded patches and grime.
/// Paint roughness ~0.35 - 0.5.
pub fn painted_steel(res: u32, paint: [f32; 3], wear: f32) -> TextureSet {
    const T: f32 = 1.5;
    let n = res.max(16);
    let texel = T / n as f32;
    let wear = wear.clamp(0.0, 1.0);
    let paint = paint.map(|p| p.clamp(0.02, 0.9));
    // Smooth layers of the chip fields, sampled from a coarse grid.
    let warp = Field::<3>::new(n / 2, |u, v| {
        [
            fbm(u, v, 4, 4, 3, 0.5, 501),
            fbm(u, v, 4, 4, 3, 0.5, 502),
            fbm(u, v, 3, 3, 3, 0.5, 506),
        ]
    });
    let big = |u: f32, v: f32| {
        let w = warp.at(u, v);
        fbm(u + 0.06 * w[0], v + 0.06 * w[1], 5, 5, 6, 0.55, 503)
            + 0.18 * fbm(u, v, 40, 40, 3, 0.5, 504)
    };
    let small = |u: f32, v: f32| fbm(u, v, 16, 16, 4, 0.55, 505) + 0.25 * warp.at(u, v)[2];
    let big_thr = quantile(big, 1.0 - (0.004 + 0.3 * wear.powf(1.3)));
    let small_thr = quantile(small, 1.0 - (0.004 + 0.07 * wear));
    let scratches = Strokes::thin(
        16,
        scratch_strokes(
            &mut Rng(51),
            (20.0 + 260.0 * wear) as usize,
            T,
            (0.03, 0.25),
            0.5,
        ),
        n,
    );
    let low = Field::<4>::new(n / 2, |u, v| {
        [
            fbm(u, v, 3, 3, 4, 0.5, 507),
            fbm(u, v, 6, 6, 3, 0.5, 508),
            fbm(u, v, 2, 3, 3, 0.5, 509),
            fbm(u, v, 8, 8, 3, 0.5, 510),
        ]
    });
    let peel_f = (n / 3).max(8) as i32;
    let aa = 0.012;

    let mut cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 511);
        // Chip depth: <0 paint, then a primer band, then bare steel, rust in old middles.
        let chip = (big(u, v) - big_thr).max(0.7 * (small(u, v) - small_thr));
        let mut scratch = 0.0f32;
        let mut deep = 0.0f32;
        scratches.visit(u, v, |s, d, t| {
            let cov = line_cov(s.hw, d, n) * sstep(0.0, 0.1, t) * sstep(1.0, 0.85, t);
            scratch = scratch.max(cov * s.k);
            if s.k > 0.75 {
                deep = deep.max(cov);
            }
        });
        let primer = sat(chip / aa + 0.5).max(scratch);
        let steel = sat((chip - 0.03) / aa + 0.5).max(deep);
        let rust =
            sstep(0.06, 0.16, chip + 0.05 * l[3]) * sstep(0.3, 0.8, 0.75 * wear + 0.4 * l[1]);

        // Paint: orange peel, slight fade/chalking, grime.
        let peel = perlin(u * peel_f as f32, v * peel_f as f32, peel_f, peel_f, 512);
        let fade = sstep(0.0, 0.6, l[0]) * (0.3 + 0.5 * wear);
        let mut pc = scale3(paint, 1.0 + 0.05 * l[1] + 0.02 * grain);
        pc = mix3(pc, scale3(add3(pc, [0.08; 3]), 1.1), 0.25 * fade);
        let grime = sstep(-0.1, 0.5, l[2]) * (0.15 + 0.25 * wear);
        pc = mix3(pc, mul3(pc, [0.62, 0.6, 0.57]), grime);
        let mut c = pc;
        let mut r = 0.36 + 0.04 * l[1] + 0.12 * fade + 0.08 * grime + 0.02 * grain;
        let mut m = 0.0;
        let mut h = 0.00035 + 0.000012 * peel;

        let primer_c = scale3([0.27, 0.27, 0.26], 1.0 + 0.06 * grain);
        c = mix3(c, primer_c, primer);
        r = lerp(r, 0.7, primer);
        h = lerp(h, 0.00012, primer);
        let bare = scale3([0.42, 0.42, 0.43], 1.0 + 0.08 * l[1] + 0.06 * grain);
        c = mix3(c, bare, steel);
        r = lerp(r, 0.52 + 0.1 * l[3] + 0.05 * grain, steel);
        m = lerp(m, 1.0, steel);
        h = lerp(h, 0.0, steel);
        let rc = mix3(
            [0.13, 0.05, 0.025],
            [0.34, 0.14, 0.05],
            sat(0.5 + 1.2 * l[3] + 0.6 * grain),
        );
        let rust = rust * steel;
        c = mix3(c, rc, rust);
        r = lerp(r, 0.88, rust);
        m = lerp(m, 0.0, rust);
        h += 0.00005 * rust * grain;
        Px {
            h,
            c,
            r: r.clamp(0.25, 0.95),
            m,
            ao: 1.0,
            aux: rust,
        }
    });

    // Rust runs: carry rust downward (+V) with a decaying trail, broken into streaks.
    let n_us = n as usize;
    let decay = (-texel / 0.2).exp();
    let mut trail = vec![0.0f32; n_us * n_us];
    for x in 0..n_us {
        let mut s = 0.0f32;
        for y in 0..2 * n_us {
            let i = (y % n_us) * n_us + x;
            s = (s * decay).max(cv.px[i].aux);
            if y >= n_us {
                trail[i] = s;
            }
        }
    }
    let trail = blur(&trail, n_us, 2, 2);
    let strength = 0.3 + 0.7 * wear;
    cv.modify(|p, i| {
        let (u, v) = cv_uv(i, n_us);
        let streak = sstep(-0.1, 0.35, fbm(u, v, 45, 3, 3, 0.5, 513));
        let k = sat(trail[i] * streak * strength) * (1.0 - p.m) * (1.0 - p.aux);
        // A stain: darkens and browns the paint, opaque only where the run is heavy.
        let k = sstep(0.05, 0.7, k);
        p.c = mix3(p.c, mul3(p.c, [0.75, 0.55, 0.4]), k);
        p.c = mix3(p.c, [0.22, 0.095, 0.04], 0.55 * k * k);
        p.r = lerp(p.r, 0.7, 0.5 * k);
    });
    cv.normal_gain = 2.5;
    cv.finish(&[(2, 0.00012, 0.35), (6, 0.0003, 0.2)])
}

/// Bare forged steel for the wrecking ball, chain and axles. `tile_m = 1`. Oxidized steel with
/// shallow overlapping hammer dents, blue-black mill scale left in soft mottled patches (~40 %),
/// polished bright on worn ridges, fine handling scratches, and small rust patches (~8 %) that
/// collect in the dents with clustered pits and a brown film around them. Metallic 1 (rust and
/// its film lower it), roughness ~0.3 - 0.8.
pub fn forged_steel(res: u32) -> TextureSet {
    const T: f32 = 1.0;
    let n = res.max(16);
    let texel = T / n as f32;
    let low = Field::<7>::new(n / 2, |u, v| {
        let wu = fbm(u, v, 3, 3, 2, 0.5, 701);
        let wv = fbm(u, v, 3, 3, 2, 0.5, 702);
        [
            fbm(u, v, 3, 3, 4, 0.5, 703),
            fbm(u, v, 10, 10, 3, 0.5, 704),
            fbm(u + 0.08 * wu, v + 0.08 * wv, 6, 6, 4, 0.55, 705),
            fbm(u, v, 12, 12, 3, 0.5, 706),
            fbm(u, v, 2, 2, 3, 0.5, 707),
            fbm(u + 0.1 * wv, v + 0.1 * wu, 5, 5, 3, 0.55, 708),
            fbm(u, v, 24, 24, 3, 0.6, 709),
        ]
    });
    let scale_f = |l: &[f32; 7]| l[5] + 0.3 * l[6];
    // ~40 % of the surface keeps its mill scale, ~8 % carries rust.
    let q_scale = quantile(|u, v| scale_f(&low.at(u, v)), 0.4);
    let q_rust = quantile(|u, v| low.at(u, v)[2] + 0.3 * low.at(u, v)[6], 0.9);
    let scratches = Strokes::thin(
        16,
        scratch_strokes(&mut Rng(72), 700, T, (0.006, 0.05), 0.6),
        n,
    );
    let sand_f = (n / 3).max(8) as i32;

    let mut cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 710);
        let sand = perlin(u * sand_f as f32, v * sand_f as f32, sand_f, sand_f, 711);
        // Hammered surface: overlapping shallow dents (dent centers low, ridges between).
        let d1 = worley(u, v, 18, 0.95, 712).d;
        let d2 = worley(u, v, 37, 0.95, 713).d;
        let dent = (1.0 - d1 * d1).max(0.0) + 0.45 * (1.0 - d2 * d2).max(0.0);
        let ridge = sat(1.0 - dent / 1.1);
        let s = scale_f(&l) - 0.1 * (ridge - 0.5);
        let scale = sstep(q_scale + 0.08, q_scale - 0.1, s);
        // Polished where handling and impacts rubbed the ridges.
        let worn = sstep(0.55, 0.9, ridge) * sstep(q_scale + 0.05, q_scale + 0.3, s);

        let warm = sstep(-0.2, 0.5, l[4]) * 0.5;
        let base = mix3([0.3, 0.3, 0.31], [0.33, 0.3, 0.27], warm);
        let base = scale3(
            base,
            1.0 + 0.1 * l[0] + 0.08 * l[1] + 0.06 * l[6] + 0.03 * grain,
        );
        let bright = scale3([0.5, 0.5, 0.51], 1.0 + 0.03 * grain);
        let scale_c = scale3(
            [0.19, 0.195, 0.21],
            1.0 + 0.25 * l[1] + 0.15 * l[6] + 0.1 * sand + 0.05 * grain,
        );
        let mut c = mix3(mix3(base, bright, worn), scale_c, scale);
        let mut r = lerp(
            lerp(0.5 + 0.07 * l[3] + 0.03 * grain, 0.32, worn),
            0.62 + 0.05 * l[1] + 0.03 * sand,
            scale,
        );
        // Fine handling scratches: catch the light, barely change the color.
        let mut scratch = 0.0f32;
        scratches.visit(u, v, |st, d, t| {
            let k = line_cov(st.hw, d, n) * sstep(0.0, 0.2, t) * sstep(1.0, 0.8, t);
            scratch = scratch.max(k * st.k);
        });
        c = mix3(c, bright, 0.4 * scratch);
        r -= 0.18 * scratch;

        // Rust: small patches that collect in the dents, a brown film bleeding around them,
        // clustered pits inside.
        let rs = l[2] + 0.3 * l[6] + 0.06 * sand + 0.04 * grain;
        let core = sstep(q_rust, q_rust + 0.1, rs + 0.06 * (0.5 - ridge));
        let film = sstep(q_rust - 0.12, q_rust, rs) * (1.0 - worn);
        let mut pit_k = 0.0f32;
        if core > 0.05 {
            let pit = worley(u, v, 80, 0.9, 714);
            if unit(hash(pit.id ^ 9)) < 0.7 * core {
                let pr = lerp(0.0005, 0.0014, unit(hash(pit.id ^ 10)));
                pit_k = sat((pr - pit.d * T / 80.0) / texel + 0.5);
            }
        }
        let rc = mix3(
            [0.09, 0.04, 0.02],
            [0.24, 0.1, 0.04],
            sat(0.45 + 1.2 * l[1] + 0.6 * grain),
        );
        c = mix3(c, mul3(c, [0.85, 0.62, 0.45]), 0.6 * film);
        r = lerp(r, 0.7, 0.5 * film);
        let rust = sat(core + 0.8 * pit_k);
        c = mix3(c, rc, rust);
        r = lerp(r, 0.82, rust);
        let m = lerp(lerp(1.0, 0.6, film), 0.15, rust);
        c = scale3(c, 1.0 - 0.6 * pit_k);
        let h = -0.0008 * dent + 0.00004 * sand + 0.00005 * scale - 0.0004 * pit_k
            + 0.00004 * rust * grain
            - 0.00002 * scratch;
        Px {
            h,
            c,
            r: r.clamp(0.25, 0.85),
            m,
            ao: 1.0 - 0.5 * pit_k,
            aux: 0.0,
        }
    });
    cv.normal_gain = 2.0;
    cv.finish(&[(3, 0.0002, 0.35), (10, 0.0006, 0.3)])
}

/// Steel tread plate ("diamond plate") for the turntable deck. `tile_m = 1`: 20 × 20 raised
/// lugs (50 mm pitch, 2 mm high) in the classic alternating diagonal pattern, polished tops,
/// dirt and a little rust packed around the lug bases. Metallic except for the dirt.
pub fn diamond_plate(res: u32) -> TextureSet {
    const T: f32 = 1.0;
    const N: f32 = 20.0;
    const LA: f32 = 0.39; // lug half length, cells
    const WB: f32 = 0.11; // lug half width, cells
    const H: f32 = 0.002;
    let n = res.max(16);
    let pw = N / n as f32; // one pixel in cells
    let low = Field::<5>::new(n / 2, |u, v| {
        [
            fbm(u, v, 3, 3, 4, 0.5, 801),
            fbm(u, v, 8, 8, 3, 0.5, 802),
            fbm(u, v, 5, 5, 4, 0.5, 803),
            fbm(u, v, 2, 40, 3, 0.5, 804),
            fbm(u, v, 12, 12, 3, 0.5, 805),
        ]
    });
    let scratches = Strokes::thin(
        16,
        scratch_strokes(&mut Rng(81), 220, T, (0.03, 0.2), 0.6),
        n,
    );

    let mut cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 806);
        let (x, y) = (u * N, v * N);
        let (cx, cy) = (x.floor(), y.floor());
        let (lx, ly) = (x - cx - 0.5, y - cy - 0.5);
        let even = ((cx as i32 + cy as i32) & 1) == 0;
        let (a, b) = if even {
            ((lx + ly) * FRAC_1_SQRT_2, (ly - lx) * FRAC_1_SQRT_2)
        } else {
            ((lx - ly) * FRAC_1_SQRT_2, (lx + ly) * FRAC_1_SQRT_2)
        };
        let s = (a / LA).abs().min(1.0);
        let w = WB * (1.0 - s * s).max(0.0).powf(0.55);
        let inside = (w - b.abs()).min((LA - a.abs()) * 0.4);
        // Rounded shoulder over ~3 mm, slightly crowned top.
        let shoulder = sstep(-0.5 * pw, 0.06, inside);
        let crown = 0.88 + 0.12 * (1.0 - (b / w.max(1e-4)).powi(2)).max(0.0);
        let lug = shoulder * crown;
        let h = H * lug + 0.00005 * l[1] + 0.00002 * grain;

        let base = scale3(
            [0.48, 0.48, 0.49],
            1.0 + 0.05 * l[0] + 0.05 * l[1] + 0.03 * grain,
        );
        let mut r = 0.48 + 0.06 * l[1] + 0.04 * l[3] + 0.03 * grain;
        let top = sat((lug - 0.75) / 0.2) * (0.55 + 0.45 * sstep(-0.4, 0.3, l[2]));
        let mut c = mix3(base, [0.62, 0.62, 0.63], 0.6 * top);
        r = lerp(r, 0.3 + 0.05 * l[1], top);
        let mut sc = 0.0f32;
        scratches.visit(u, v, |st, d, t| {
            let k = line_cov(st.hw, d, n) * sstep(0.0, 0.15, t) * sstep(1.0, 0.85, t);
            sc = sc.max(k * st.k);
        });
        c = mix3(c, [0.62, 0.62, 0.63], 0.45 * sc);
        r = lerp(r, 0.32, 0.5 * sc);
        Px {
            h,
            c,
            r,
            m: 1.0,
            ..Px::default()
        }
    });
    // Dirt and rust packed around the lug bases, more in grimy patches.
    let cav = cv.cavity(3);
    let n_us = n as usize;
    cv.modify(|p, i| {
        let (u, v) = cv_uv(i, n_us);
        let l = low.at(u, v);
        let recess = sat(cav[i] / 0.0005);
        let patch = sstep(-0.3, 0.5, l[2]);
        let dirt = sat(recess * (0.45 + 0.55 * patch) * (0.75 + 0.25 * sstep(-0.3, 0.3, l[4])))
            + 0.12 * patch * sat(1.0 - p.h / (0.3 * H));
        let dirt = sat(dirt);
        let rusty = sstep(0.15, 0.45, l[4]) * dirt;
        let dirt_c = mix3([0.085, 0.075, 0.06], [0.2, 0.085, 0.035], rusty);
        p.c = mix3(p.c, dirt_c, 0.85 * dirt);
        p.r = lerp(p.r, 0.88, dirt);
        p.m = lerp(p.m, 0.0, sat(dirt * 1.2));
        p.h += 0.0002 * dirt;
    });
    cv.finish(&[(2, 0.0004, 0.45), (6, 0.0008, 0.3)])
}

/// Soft matte rubber (balls, bumpers). `tile_m = 0.5`. Light neutral albedo (about sRGB 0xcf)
/// so `base_color` sets the color: molded pebble and stipple relief (flattened and glossier
/// where rubbed), lighter matte abrasion scuffs, dark transfer smears from hard impacts, a few
/// small cuts, dust settled in the low spots. Roughness ~0.65 - 0.95.
pub fn rubber(res: u32) -> TextureSet {
    const T: f32 = 0.5;
    let n = res.max(16);
    let scuffs = Strokes::thin(
        12,
        scratch_strokes(&mut Rng(91), 90, T, (0.01, 0.07), 2.5),
        n,
    );
    let smears = Strokes::new(8, scratch_strokes(&mut Rng(92), 9, T, (0.03, 0.09), 14.0));
    let cuts = Strokes::thin(
        8,
        scratch_strokes(&mut Rng(93), 6, T, (0.004, 0.012), 1.2),
        n,
    );
    let low = Field::<4>::new(n / 2, |u, v| {
        [
            fbm(u, v, 4, 4, 4, 0.5, 904),
            fbm(u, v, 6, 6, 3, 0.5, 905),
            fbm(u, v, 12, 12, 3, 0.5, 906),
            fbm(u, v, 3, 3, 3, 0.5, 907),
        ]
    });
    let stip_f = (n as f32 / 2.6) as i32;
    let oct = octaves_below(60, 2, n);
    let mut cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 901);
        let stipple = perlin(u * stip_f as f32, v * stip_f as f32, stip_f, stip_f, 902);
        let pebble = fbm(u, v, 60, 60, oct, 0.5, 903);
        let rub = sstep(0.12, 0.42, l[1]);
        let lum =
            0.62 * (1.0 + 0.07 * l[0] + 0.04 * pebble + 0.03 * stipple + 0.03 * grain - 0.05 * rub);
        let mut c = [lum, lum, lum * 0.99];
        let mut r = 0.86 + 0.04 * l[3] + 0.03 * stipple + 0.02 * grain - 0.18 * rub;
        // Rubbing flattens the molded texture.
        let mut h = (1.0 - 0.6 * rub) * (0.00006 * stipple + 0.0001 * pebble) + 0.00001 * grain;

        // Abrasion scuffs: lighter and matte.
        let mut sc = 0.0f32;
        scuffs.visit(u, v, |s, d, t| {
            let k = line_cov(s.hw, d, n) * sstep(0.0, 0.3, t) * sstep(1.0, 0.6, t);
            sc = sc.max(k * s.k);
        });
        c = scale3(c, 1.0 + 0.16 * sc);
        r = lerp(r, 0.95, 0.7 * sc);
        h -= 0.00002 * sc;
        // Dark transfer smears from hard impacts, streaky along the stroke.
        let mut sm = 0.0f32;
        smears.visit(u, v, |s, d, t| {
            let x = d / s.hw;
            let streak = 0.5 + 0.5 * perlin(x * 6.0 + s.id as f32 * 2.3, t * 3.0, 1 << 16, 3, 908);
            let k = sstep(1.0, 0.3, x.abs()) * sstep(0.0, 0.4, t) * sstep(1.0, 0.5, t) * streak;
            sm = sm.max(k * s.k);
        });
        c = scale3(c, 1.0 - 0.45 * sm);
        r = lerp(r, 0.72, 0.5 * sm);
        // A few small cuts.
        let mut cut = 0.0f32;
        cuts.visit(u, v, |s, d, t| {
            cut = cut.max(line_cov(s.hw, d, n) * sstep(0.0, 0.2, t) * sstep(1.0, 0.8, t));
        });
        c = scale3(c, 1.0 - 0.5 * cut);
        h -= 0.0003 * cut;
        Px {
            h,
            c,
            r: r.clamp(0.6, 0.96),
            ao: 1.0 - 0.4 * cut,
            aux: sat(0.3 + 1.5 * l[2]),
            ..Px::default()
        }
    });
    // Dust settles in the low spots, more in some patches.
    cv.normal_gain = 2.0;
    let cav = cv.cavity(2);
    cv.modify(|p, i| {
        let k = sat(cav[i] / 0.00004) * (0.3 + 0.7 * p.aux);
        p.c = scale3(p.c, 1.0 + 0.12 * k);
        p.r = lerp(p.r, 0.95, 0.6 * k);
    });
    cv.finish(&[(2, 0.00004, 0.3), (6, 0.0001, 0.2)])
}

/// Crash-test-dummy skin: satin vinyl, near white (about sRGB 0xe4) so `base_color` tints it
/// (yellow, black). `tile_m = 0.5`. Orange peel, glossier hand-worn and duller scuffed
/// patches, clusters of fine scratches, dark transfer smears from impacts, grime.
/// Roughness ~0.3 - 0.65.
pub fn vinyl_skin(res: u32) -> TextureSet {
    const T: f32 = 0.5;
    let n = res.max(16);
    let scuffs = Strokes::thin(
        12,
        scratch_strokes(&mut Rng(95), 130, T, (0.006, 0.05), 0.8),
        n,
    );
    let smears = Strokes::new(8, scratch_strokes(&mut Rng(96), 7, T, (0.03, 0.1), 12.0));
    let low = Field::<5>::new(n / 2, |u, v| {
        [
            fbm(u, v, 3, 3, 4, 0.5, 953),
            fbm(u, v, 5, 5, 4, 0.55, 954),
            fbm(u, v, 8, 8, 3, 0.5, 955),
            fbm(u, v, 4, 4, 3, 0.5, 956),
            fbm(u, v, 20, 20, 2, 0.5, 957),
        ]
    });
    let oct = octaves_below(100, 2, n);
    let mut cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 951);
        // Orange peel: smooth round dimples, ~5 mm.
        let peel = fbm(u, v, 100, 100, oct, 0.3, 952);
        let grime = sstep(0.05, 0.45, l[1]);
        let gloss = sstep(0.1, 0.4, l[2]);
        let cluster = sstep(0.0, 0.35, l[3]);
        let lum = 0.78 * (1.0 + 0.03 * l[0] + 0.02 * l[4] + 0.01 * grain);
        let mut c = [lum, lum * 0.995, lum * 0.98];
        let mut r = 0.4 + 0.03 * l[0] + 0.02 * peel - 0.08 * gloss;

        // Scratches gather in scuffed patches, which are dull overall.
        let mut sc = 0.0f32;
        scuffs.visit(u, v, |s, d, t| {
            let k = line_cov(s.hw, d, n) * sstep(0.0, 0.25, t) * sstep(1.0, 0.75, t);
            sc = sc.max(k * s.k);
        });
        sc *= 0.15 + 0.85 * cluster;
        let dull = cluster * sstep(0.15, 0.45, l[4] + 0.3 * l[3]);
        c = scale3(c, 1.0 - 0.1 * sc + 0.03 * dull);
        r = lerp(lerp(r, 0.56, 0.7 * dull), 0.64, sc);
        // Dark transfer smears from impacts.
        let mut sm = 0.0f32;
        smears.visit(u, v, |s, d, t| {
            let x = d / s.hw;
            let streak = 0.5 + 0.5 * perlin(x * 5.0 + s.id as f32 * 1.7, t * 3.0, 1 << 16, 3, 958);
            let k = sstep(1.0, 0.3, x.abs()) * sstep(0.0, 0.4, t) * sstep(1.0, 0.5, t) * streak;
            sm = sm.max(k * s.k);
        });
        c = scale3(c, 1.0 - 0.4 * sm);
        r = lerp(r, 0.6, 0.6 * sm);
        c = mix3(c, mul3(c, [0.8, 0.78, 0.75]), 0.55 * grime);
        r += 0.06 * grime;
        Px {
            h: 0.00005 * peel - 0.00001 * sc - 0.000005 * sm,
            c,
            r: r.clamp(0.28, 0.72),
            ..Px::default()
        }
    });
    cv.normal_gain = 1.5;
    cv.finish(&[(2, 0.00003, 0.2)])
}

/// Worn yellow/black safety stripes painted on concrete (curb tops and faces). `tile_m = 1`:
/// 45° stripes, 4 stripe pairs per tile (each band 125 mm wide along U or V, 88 mm across).
/// Paint abraded down to the concrete in speckled traffic areas and on the high points, a few
/// flaked patches, concrete pores showing through, dirt and tire smears over it.
pub fn hazard_stripes(res: u32) -> TextureSet {
    const T: f32 = 1.0;
    const PAIRS: f32 = 4.0;
    let n = res.max(16);
    let texel = T / n as f32;
    let flakes = |u: f32, v: f32| {
        let wu = fbm(u, v, 3, 3, 3, 0.5, 1001);
        fbm(u + 0.05 * wu, v - 0.05 * wu, 6, 6, 5, 0.55, 1002)
            + 0.2 * fbm(u, v, 48, 48, 2, 0.5, 1003)
    };
    let flake_thr = quantile(flakes, 0.94);
    let low = Field::<4>::new(n / 2, |u, v| {
        [
            fbm(u, v, 3, 3, 4, 0.5, 1004),
            fbm(u, v, 12, 12, 3, 0.5, 1005),
            fbm(u, v, 5, 5, 4, 0.5, 1006),
            fbm(u, v, 3, 3, 4, 0.55, 1007),
        ]
    });
    // Rubber smudges where tires brushed the curb: 4 - 11 cm wide, 15 - 50 cm long.
    let tires = Strokes::new(8, scratch_strokes(&mut Rng(101), 5, T, (0.15, 0.5), 60.0));
    let sand_f = (n / 3).max(8) as i32;
    let cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let grain = grain(u, v, n, 1008);
        let sand = perlin(u * sand_f as f32, v * sand_f as f32, sand_f, sand_f, 1009);
        // Concrete underneath: paste, small stones, pores.
        let lum = 0.31 * (1.0 + 0.08 * l[0] + 0.07 * sand + 0.08 * grain);
        let mut conc = [lum * 1.02, lum, lum * 0.96];
        let st = worley(u, v, 40, 0.9, 1010);
        let stone_rad = lerp(0.002, 0.006, unit(hash(st.id ^ 3))) * (1.0 + 0.3 * sand);
        let stone = if unit(hash(st.id ^ 4)) < 0.4 {
            sat((stone_rad - st.d * T / 40.0) / texel + 0.5)
        } else {
            0.0
        };
        conc = mix3(
            conc,
            scale3([0.42, 0.4, 0.37], 0.8 + 0.4 * unit(hash(st.id))),
            0.8 * stone,
        );
        let pore = worley(u, v, 90, 0.9, 1011);
        let pore_k = if unit(hash(pore.id ^ 5)) < 0.18 {
            sat((0.0016 - pore.d * T / 90.0) / texel + 0.5)
        } else {
            0.0
        };
        conc = scale3(conc, 1.0 - 0.5 * pore_k);
        let conc_h = 0.00022 * sand + 0.0004 * stone - 0.0012 * pore_k + 0.00005 * grain;

        // Stripes: phase along the diagonal, edges anti-aliased and a little ragged.
        let phase = (u + v) * PAIRS + 0.004 * fbm(u, v, 60, 60, 2, 0.5, 1012);
        let f = phase.rem_euclid(1.0);
        let edge_px = T / (PAIRS * std::f32::consts::SQRT_2 * texel);
        let d_edge = f.min(1.0 - f).min((f - 0.5).abs()) * edge_px;
        let yellow = [0.7, 0.45, 0.025];
        let black = [0.028, 0.027, 0.026];
        let (a, b) = if f < 0.5 {
            (yellow, black)
        } else {
            (black, yellow)
        };
        let paint = scale3(
            mix3(b, a, sat(0.5 + d_edge)),
            1.0 + 0.05 * l[1] + 0.03 * grain,
        );

        // Abrasion: in traffic areas the paint wears off the high points first (speckled),
        // plus a few flaked patches; pores stay dark under the paint.
        let traffic = sstep(-0.15, 0.45, l[3]);
        let abrasion = 0.45 * sand + 0.3 * grain + 0.5 * stone + 0.9 * traffic - 0.4 * pore_k;
        let worn = sat((abrasion - 0.78) / 0.12 + 0.5);
        let flake = sat((flakes(u, v) - flake_thr) / 0.015 + 0.5);
        let gone = worn.max(flake);
        let thin = 0.35 * traffic;
        let mut c = mix3(paint, conc, sat(gone + thin * (1.0 - gone)));
        c = scale3(c, 1.0 - 0.35 * pore_k * (1.0 - gone));
        let mut r = lerp(
            0.5 + 0.05 * l[1] + 0.05 * traffic,
            0.88 + 0.04 * grain,
            gone,
        );
        let h = conc_h + 0.0002 * (1.0 - flake);

        // Dirt and tire smears.
        let dirt = sstep(-0.2, 0.5, l[2]);
        c = mix3(c, mul3(c, [0.62, 0.6, 0.57]), 0.45 * dirt);
        r += 0.05 * dirt;
        let mut tire = 0.0f32;
        tires.visit(u, v, |s, d, t| {
            let x = d / s.hw;
            let streak =
                0.45 + 0.55 * perlin(x * 5.0 + s.id as f32 * 1.3, t * 4.0, 1 << 16, 4, 1013);
            let k = sstep(1.0, 0.3, x.abs()) * sstep(0.0, 0.3, t) * sstep(1.0, 0.7, t);
            tire = tire.max(k * s.k * sat(streak));
        });
        c = scale3(c, 1.0 - 0.45 * tire);
        Px {
            h,
            c,
            r: r.clamp(0.45, 0.95),
            ao: 1.0 - 0.4 * pore_k,
            ..Px::default()
        }
    });
    cv.finish(&[(2, 0.0003, 0.4), (6, 0.0008, 0.3)])
}

/// Short lawn seen from a few meters up (around the yard). `tile_m = 4`: dense blades with
/// shadowed gaps between them, dry yellow patches, lusher dark patches, clover and a little
/// bare soil. Roughness ~0.85 - 0.95.
pub fn lawn(res: u32) -> TextureSet {
    const T: f32 = 4.0;
    const G: i32 = 128; // blade cells per tile side (31 mm cells)
    const PER: usize = 5;
    let n = res.max(16);
    // Blades, precomputed per cell: origin, direction × length, height, tone (cell units).
    let blades: Vec<[f32; 6]> = (0..G * G)
        .flat_map(|cell| {
            let (kx, ky) = (cell % G, cell / G);
            (0..PER as u32).map(move |b| {
                let hsh = hash2(kx, ky, 1105 + b * 7919);
                let ang = unit(hash(hsh ^ 2)) * TAU;
                let len = 0.7 + 0.9 * unit(hash(hsh ^ 3));
                [
                    unit(hsh),
                    unit(hash(hsh ^ 1)),
                    ang.cos() * len,
                    ang.sin() * len,
                    0.3 + 0.7 * unit(hash(hsh ^ 4)),
                    unit(hash(hsh ^ 5)),
                ]
            })
        })
        .collect();
    let low = Field::<4>::new(n / 2, |u, v| {
        [
            fbm(u, v, 3, 3, 5, 0.55, 1101),
            fbm(u, v, 6, 6, 4, 0.5, 1102),
            fbm(u, v, 20, 20, 3, 0.5, 1103),
            fbm(u, v, 2, 2, 3, 0.5, 1104),
        ]
    });
    let cv = Canvas::new(n, T, |u, v| {
        let l = low.at(u, v);
        let (gx, gy) = (u * G as f32, v * G as f32);
        let (cx, cy) = (gx.floor() as i32, gy.floor() as i32);
        // Topmost blade covering this pixel.
        let mut best_h = -1.0f32;
        let mut best = (0.0f32, 0.0f32);
        for j in -1..=1 {
            for i in -1..=1 {
                let cell = ((cy + j).rem_euclid(G) * G + (cx + i).rem_euclid(G)) as usize;
                for bl in &blades[cell * PER..(cell + 1) * PER] {
                    let (px, py) = (gx - (cx + i) as f32 - bl[0], gy - (cy + j) as f32 - bl[1]);
                    let len2 = bl[2] * bl[2] + bl[3] * bl[3];
                    let t = ((px * bl[2] + py * bl[3]) / len2).clamp(0.0, 1.0);
                    let (ex, ey) = (px - bl[2] * t, py - bl[3] * t);
                    let width = 0.19 * (1.0 - 0.65 * t);
                    let d2 = ex * ex + ey * ey;
                    if d2 < width * width {
                        // Blades rise from the base (t = 0) to the tip.
                        let hh = bl[4] * (0.4 + 0.6 * t) * (1.0 - 0.3 * d2.sqrt() / width);
                        if hh > best_h {
                            best_h = hh;
                            best = (t, bl[5]);
                        }
                    }
                }
            }
        }
        let (t, tone) = best;
        let dry = sstep(0.1, 0.5, l[0] + 0.3 * (tone - 0.5));
        let lush = sstep(0.0, 0.4, -l[1]);
        let green = mix3([0.05, 0.11, 0.022], [0.09, 0.16, 0.03], tone);
        let mut g = mix3(green, [0.2, 0.18, 0.07], 0.7 * dry);
        g = mix3(g, [0.035, 0.085, 0.02], 0.6 * lush);
        let blade_c = scale3(g, 0.7 + 0.5 * t);
        let soil = [0.06, 0.05, 0.035];
        let thatch = [0.03, 0.05, 0.015];
        let bare = sstep(0.4, 0.65, l[3] * 0.6 + l[2] * 0.6);
        let (c, h, ao) = if best_h < 0.0 {
            (mix3(thatch, soil, bare), 0.0, 0.3)
        } else {
            let k = 1.0 - 0.6 * bare;
            (
                mix3(soil, blade_c, k),
                0.012 * best_h * k,
                0.45 + 0.55 * best_h,
            )
        };
        // Clover patches: rounder, bluer green leaves.
        let clover = sstep(0.4, 0.6, l[2]) * sstep(0.0, 0.3, l[1]);
        let c = mix3(
            c,
            scale3([0.05, 0.12, 0.04], 0.8 + 0.4 * best_h.max(0.0)),
            0.6 * clover,
        );
        Px {
            h,
            c,
            r: 0.9 + 0.05 * (1.0 - best_h.max(0.0)),
            ao,
            ..Px::default()
        }
    });
    cv.finish(&[(2, 0.004, 0.35), (6, 0.008, 0.25)])
}

// =============================================================================================
// Canvas: per-pixel fields and the final maps
// =============================================================================================

/// One texel of a material before packing.
#[derive(Clone, Copy)]
struct Px {
    /// Height above the mean surface, meters.
    h: f32,
    /// Linear albedo.
    c: [f32; 3],
    /// Perceptual roughness.
    r: f32,
    /// Metallic.
    m: f32,
    /// Baked occlusion (multiplied with the cavity term), 0..1.
    ao: f32,
    /// A material-specific mask for post passes (not packed).
    aux: f32,
}

impl Default for Px {
    fn default() -> Self {
        Self {
            h: 0.0,
            c: [0.5; 3],
            r: 0.8,
            m: 0.0,
            ao: 1.0,
            aux: 0.0,
        }
    }
}

struct Canvas {
    n: usize,
    tile_m: f32,
    /// Multiplies the height slopes when deriving normals (1 = physical).
    normal_gain: f32,
    px: Vec<Px>,
}

/// Tile coordinates of pixel `i` (centers, 0..1).
fn cv_uv(i: usize, n: usize) -> (f32, f32) {
    let inv = 1.0 / n as f32;
    (((i % n) as f32 + 0.5) * inv, ((i / n) as f32 + 0.5) * inv)
}

impl Canvas {
    /// Evaluate `f(u, v)` at every pixel center (u, v in 0..1, rows top to bottom).
    fn new(res: u32, tile_m: f32, f: impl Fn(f32, f32) -> Px + Sync) -> Self {
        let n = res as usize;
        let mut px = vec![Px::default(); n * n];
        let inv = 1.0 / n as f32;
        par_rows(&mut px, n, |y, row| {
            let v = (y as f32 + 0.5) * inv;
            for (x, p) in row.iter_mut().enumerate() {
                *p = f((x as f32 + 0.5) * inv, v);
            }
        });
        Self {
            n,
            tile_m,
            normal_gain: 1.0,
            px,
        }
    }

    fn heights(&self) -> Vec<f32> {
        self.px.iter().map(|p| p.h).collect()
    }

    /// How far each texel lies below its surroundings (blur radius in pixels), meters.
    fn cavity(&self, radius: usize) -> Vec<f32> {
        let h = self.heights();
        let b = blur(&h, self.n, radius, 2);
        b.iter().zip(&h).map(|(b, h)| b - h).collect()
    }

    /// Change every texel in parallel; `f` gets the texel and its index.
    fn modify(&mut self, f: impl Fn(&mut Px, usize) + Sync) {
        let n = self.n;
        par_rows(&mut self.px, n, |y, row| {
            for (x, p) in row.iter_mut().enumerate() {
                f(p, y * n + x);
            }
        });
    }

    /// Pack into images. `cavity` lists (blur radius px, depth m, strength): texels lying
    /// `depth` below the blurred surface get their occlusion reduced by `strength`.
    fn finish(self, cavity: &[(usize, f32, f32)]) -> TextureSet {
        let n = self.n;
        let h = self.heights();
        let blurred: Vec<(Vec<f32>, f32, f32)> = cavity
            .iter()
            .map(|&(r, depth, k)| (blur(&h, n, r, 2), depth, k))
            .collect();
        let slope = self.normal_gain * n as f32 / (2.0 * self.tile_m);
        let lut = srgb_lut();

        let mut albedo = vec![0u8; n * n * 4];
        par_rows(&mut albedo, n * 4, |y, row| {
            for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let c = self.px[y * n + x].c;
                for (o, &v) in out.iter_mut().zip(&c) {
                    *o = lut[(sat(v) * (LUT_SIZE - 1) as f32 + 0.5) as usize];
                }
                out[3] = 255;
            }
        });
        let mut normal = vec![0u8; n * n * 4];
        par_rows(&mut normal, n * 4, |y, row| {
            let (up, down) = ((y + n - 1) % n, (y + 1) % n);
            for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let (left, right) = ((x + n - 1) % n, (x + 1) % n);
                let dx = (h[y * n + right] - h[y * n + left]) * slope;
                let dy = (h[down * n + x] - h[up * n + x]) * slope;
                // Raised = brighter on the side facing the light: X = -dh/du, Y = +dh/dv
                // (+Y points up the image, toward -V).
                let inv = 1.0 / (dx * dx + dy * dy + 1.0).sqrt();
                out[0] = unorm(-dx * inv * 0.5 + 0.5);
                out[1] = unorm(dy * inv * 0.5 + 0.5);
                out[2] = unorm(inv * 0.5 + 0.5);
                out[3] = 255;
            }
        });
        let mut orm = vec![0u8; n * n * 4];
        par_rows(&mut orm, n * 4, |y, row| {
            for (x, out) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let i = y * n + x;
                let p = &self.px[i];
                let mut ao = p.ao;
                for (b, depth, k) in &blurred {
                    ao *= 1.0 - k * sat((b[i] - h[i]) / depth);
                }
                out[0] = unorm(ao.clamp(0.05, 1.0));
                out[1] = unorm(p.r.clamp(0.02, 1.0));
                out[2] = unorm(sat(p.m));
                out[3] = 255;
            }
        });
        let size = n as u32;
        TextureSet {
            albedo: Image::new(size, size, albedo, true),
            normal: Image::new(size, size, normal, false),
            orm: Image::new(size, size, orm, false),
            tile_m: self.tile_m,
        }
    }
}

const LUT_SIZE: usize = 4096;

/// Linear 0..1 (in `LUT_SIZE` steps) to 8-bit sRGB.
fn srgb_lut() -> Vec<u8> {
    (0..LUT_SIZE)
        .map(|i| {
            let l = i as f32 / (LUT_SIZE - 1) as f32;
            let s = if l <= 0.003_130_8 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            unorm(s)
        })
        .collect()
}

fn unorm(x: f32) -> u8 {
    (sat(x) * 255.0 + 0.5) as u8
}

/// A small periodic multi-channel field, evaluated on a coarse grid and sampled bilinearly.
/// Used for the smooth, low-frequency layers (mottling, masks, warps), which would be wasteful
/// to evaluate per pixel.
struct Field<const K: usize> {
    n: usize,
    data: Vec<[f32; K]>,
}

impl<const K: usize> Field<K> {
    fn new(res: u32, f: impl Fn(f32, f32) -> [f32; K] + Sync) -> Self {
        let n = res.max(8) as usize;
        let mut data = vec![[0.0; K]; n * n];
        let inv = 1.0 / n as f32;
        par_rows(&mut data, n, |y, row| {
            let v = (y as f32 + 0.5) * inv;
            for (x, p) in row.iter_mut().enumerate() {
                *p = f((x as f32 + 0.5) * inv, v);
            }
        });
        Self { n, data }
    }

    fn at(&self, u: f32, v: f32) -> [f32; K] {
        let n = self.n;
        let x = u * n as f32 - 0.5;
        let y = v * n as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = (x - x0, y - y0);
        let x0 = (x0 as i32).rem_euclid(n as i32) as usize;
        let y0 = (y0 as i32).rem_euclid(n as i32) as usize;
        let (x1, y1) = ((x0 + 1) % n, (y0 + 1) % n);
        let (a, b) = (&self.data[y0 * n + x0], &self.data[y0 * n + x1]);
        let (c, d) = (&self.data[y1 * n + x0], &self.data[y1 * n + x1]);
        std::array::from_fn(|k| lerp(lerp(a[k], b[k], tx), lerp(c[k], d[k], tx), ty))
    }
}

// =============================================================================================
// Parallel helpers
// =============================================================================================

fn threads() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16)
}

/// Run `f(first_row, chunk)` over chunks of `rows_per` rows of a row-major buffer on all cores.
fn par_chunks<T: Send>(
    data: &mut [T],
    row_len: usize,
    rows_per: usize,
    f: impl Fn(usize, &mut [T]) + Sync,
) {
    let chunks = Mutex::new(data.chunks_mut(row_len * rows_per).enumerate());
    std::thread::scope(|s| {
        for _ in 0..threads() {
            s.spawn(|| {
                loop {
                    let next = chunks.lock().unwrap_or_else(|e| e.into_inner()).next();
                    let Some((i, chunk)) = next else { break };
                    f(i * rows_per, chunk);
                }
            });
        }
    });
}

/// Run `f(y, row)` for every row of a row-major buffer on all cores.
fn par_rows<T: Send>(data: &mut [T], row_len: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    let rows = data.len() / row_len.max(1);
    let rows_per = (rows / (threads() * 6)).max(1);
    par_chunks(data, row_len, rows_per, |y0, chunk| {
        for (k, row) in chunk.chunks_mut(row_len).enumerate() {
            f(y0 + k, row);
        }
    });
}

/// Wrapping box blur of an `n`×`n` field: radius `r` pixels, applied `passes` times (2 ≈ tent,
/// 3 ≈ Gaussian).
fn blur(src: &[f32], n: usize, r: usize, passes: usize) -> Vec<f32> {
    let r = r.min(n / 2 - 1);
    let mut a = src.to_vec();
    if r == 0 {
        return a;
    }
    let mut b = vec![0.0f32; n * n];
    let inv = 1.0 / (2 * r + 1) as f32;
    let rows_per = (n / (threads() * 4)).max(1);
    for _ in 0..passes {
        par_rows(&mut b, n, |y, row| {
            let s = &a[y * n..(y + 1) * n];
            let mut acc: f32 = (0..=2 * r).map(|k| s[(k + n - r) % n]).sum();
            for (x, out) in row.iter_mut().enumerate() {
                *out = acc * inv;
                acc += s[(x + r + 1) % n] - s[(x + n - r) % n];
            }
        });
        par_chunks(&mut a, n, rows_per, |y0, chunk| {
            let mut acc = vec![0.0f32; n];
            for k in 0..=2 * r {
                let yy = (y0 + k + n - r) % n;
                for (acc, s) in acc.iter_mut().zip(&b[yy * n..(yy + 1) * n]) {
                    *acc += s;
                }
            }
            for (k, row) in chunk.chunks_mut(n).enumerate() {
                let y = y0 + k;
                let add = (y + r + 1) % n;
                let sub = (y + n - r) % n;
                for (x, out) in row.iter_mut().enumerate() {
                    *out = acc[x] * inv;
                    acc[x] += b[add * n + x] - b[sub * n + x];
                }
            }
        });
    }
    a
}

// =============================================================================================
// Noise
// =============================================================================================

/// 16 unit gradients, 22.5° apart and off the axes.
const GRAD: [[f32; 2]; 16] = [
    [0.980_785, 0.195_090],
    [0.831_470, 0.555_570],
    [0.555_570, 0.831_470],
    [0.195_090, 0.980_785],
    [-0.195_090, 0.980_785],
    [-0.555_570, 0.831_470],
    [-0.831_470, 0.555_570],
    [-0.980_785, 0.195_090],
    [-0.980_785, -0.195_090],
    [-0.831_470, -0.555_570],
    [-0.555_570, -0.831_470],
    [-0.195_090, -0.980_785],
    [0.195_090, -0.980_785],
    [0.555_570, -0.831_470],
    [0.831_470, -0.555_570],
    [0.980_785, -0.195_090],
];

#[inline]
fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x21f0_aaad);
    x ^= x >> 15;
    x = x.wrapping_mul(0x735a_2d97);
    x ^= x >> 15;
    x
}

#[inline]
fn hash2(x: i32, y: i32, seed: u32) -> u32 {
    hash(
        (x as u32).wrapping_mul(0x8da6_b343)
            ^ (y as u32).wrapping_mul(0xd816_3841)
            ^ seed.wrapping_mul(0xcb1a_b31f),
    )
}

/// Uniform 0..1 from a hash.
#[inline]
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Deterministic random numbers for placing features.
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x9e37_79b9);
        unit(hash(self.0))
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        lerp(a, b, self.next())
    }
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Periodic 2D gradient noise, about -1..1. `x, y` in lattice cells; the pattern repeats every
/// `px` × `py` cells.
fn perlin(x: f32, y: f32, px: i32, py: i32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (x - x0, y - y0);
    let ix = (x0 as i32).rem_euclid(px);
    let iy = (y0 as i32).rem_euclid(py);
    let ix1 = if ix + 1 >= px { 0 } else { ix + 1 };
    let iy1 = if iy + 1 >= py { 0 } else { iy + 1 };
    let g = |cx: i32, cy: i32, dx: f32, dy: f32| {
        let gr = GRAD[(hash2(cx, cy, seed) & 15) as usize];
        gr[0] * dx + gr[1] * dy
    };
    let n00 = g(ix, iy, tx, ty);
    let n10 = g(ix1, iy, tx - 1.0, ty);
    let n01 = g(ix, iy1, tx, ty - 1.0);
    let n11 = g(ix1, iy1, tx - 1.0, ty - 1.0);
    let (sx, sy) = (fade(tx), fade(ty));
    lerp(lerp(n00, n10, sx), lerp(n01, n11, sx), sy) * 1.4
}

/// Fractal noise in tile coordinates (u, v in 0..1), periodic over the tile: `fx` × `fy` cells
/// in the first octave (anisotropic when they differ), doubling per octave. About -1..1, most
/// values within ±0.5.
fn fbm(u: f32, v: f32, fx: i32, fy: i32, octaves: u32, gain: f32, seed: u32) -> f32 {
    let (mut cx, mut cy) = (fx.max(1), fy.max(1));
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for o in 0..octaves.max(1) {
        sum += amp
            * perlin(
                u * cx as f32,
                v * cy as f32,
                cx,
                cy,
                seed.wrapping_add(o.wrapping_mul(0x9e37_79b9)),
            );
        norm += amp;
        amp *= gain;
        cx *= 2;
        cy *= 2;
    }
    sum / norm
}

/// How many fbm octaves starting at `base` cells per tile stay below the texture's Nyquist
/// rate (at least 1, at most `wanted`).
fn octaves_below(base: i32, wanted: u32, res: u32) -> u32 {
    let mut o = 1;
    let mut f = base * 2;
    while o < wanted && f as f32 <= res as f32 / 2.5 {
        o += 1;
        f *= 2;
    }
    o
}

/// Nearest feature point of a periodic jittered grid (`n` × `n` cells per tile).
#[derive(Clone, Copy)]
struct Cell {
    /// Distance to the feature point, in cells.
    d: f32,
    /// Hash of the (wrapped) cell, for per-cell random values.
    id: u32,
}

fn worley(u: f32, v: f32, n: i32, jitter: f32, seed: u32) -> Cell {
    let (x, y) = (u * n as f32, v * n as f32);
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (ix, iy) = (fx as i32, fy as i32);
    let mut best = Cell { d: 9.0, id: 0 };
    for j in -1..=1 {
        for i in -1..=1 {
            let h = hash2((ix + i).rem_euclid(n), (iy + j).rem_euclid(n), seed);
            let ox = 0.5 + jitter * (unit(h) - 0.5);
            let oy = 0.5 + jitter * (unit(hash(h)) - 0.5);
            let (dx, dy) = (i as f32 + ox - tx, j as f32 + oy - ty);
            let d = dx * dx + dy * dy;
            if d < best.d {
                best = Cell { d, id: h };
            }
        }
    }
    best.d = best.d.sqrt();
    best
}

/// Distance (in cells) to the nearest border of a periodic Voronoi diagram, and the id of the
/// cell the point is in.
fn voronoi_edge(u: f32, v: f32, n: i32, jitter: f32, seed: u32) -> (f32, u32) {
    let (x, y) = (u * n as f32, v * n as f32);
    let (fx, fy) = (x.floor(), y.floor());
    let (tx, ty) = (x - fx, y - fy);
    let (ix, iy) = (fx as i32, fy as i32);
    let point = |i: i32, j: i32| {
        let h = hash2((ix + i).rem_euclid(n), (iy + j).rem_euclid(n), seed);
        let ox = 0.5 + jitter * (unit(h) - 0.5);
        let oy = 0.5 + jitter * (unit(hash(h)) - 0.5);
        (i as f32 + ox - tx, j as f32 + oy - ty, h)
    };
    let mut best = (9.0f32, 0i32, 0i32, 0.0f32, 0.0f32, 0u32);
    for j in -1..=1 {
        for i in -1..=1 {
            let (dx, dy, h) = point(i, j);
            let d = dx * dx + dy * dy;
            if d < best.0 {
                best = (d, i, j, dx, dy, h);
            }
        }
    }
    let (_, bi, bj, mx, my, id) = best;
    let mut edge = 9.0f32;
    for j in -1..=1 {
        for i in -1..=1 {
            if i == 0 && j == 0 {
                continue;
            }
            let (dx, dy, _) = point(bi + i, bj + j);
            let (ex, ey) = (dx - mx, dy - my);
            let len2 = ex * ex + ey * ey;
            if len2 > 1e-6 {
                let d = (0.5 * (mx + dx) * ex + 0.5 * (my + dy) * ey) / len2.sqrt();
                edge = edge.min(d);
            }
        }
    }
    (edge, id)
}

/// A wrapped offset from `c` to `p` along one tile axis, in -0.5..0.5.
#[inline]
fn wrap_d(p: f32, c: f32) -> f32 {
    let d = p - c;
    d - d.round()
}

/// Per-pixel white noise in -0.5..0.5 (sand grains, speckle).
#[inline]
fn grain(u: f32, v: f32, n: u32, seed: u32) -> f32 {
    unit(hash2((u * n as f32) as i32, (v * n as f32) as i32, seed)) - 0.5
}

/// Smallest quantile `q` of `f` over the tile (sampled on a 96 × 96 grid), to turn a fractal
/// field into a mask that covers a known fraction of the surface.
fn quantile(f: impl Fn(f32, f32) -> f32 + Sync, q: f32) -> f32 {
    const S: usize = 96;
    let mut v = vec![0.0f32; S * S];
    par_rows(&mut v, S, |y, row| {
        for (x, o) in row.iter_mut().enumerate() {
            *o = f((x as f32 + 0.37) / S as f32, (y as f32 + 0.61) / S as f32);
        }
    });
    v.sort_by(f32::total_cmp);
    v[((q.clamp(0.0, 1.0) * (S * S - 1) as f32) as usize).min(S * S - 1)]
}

// =============================================================================================
// Scattered features
// =============================================================================================

/// A knot in a board: ellipse radii in meters, how far it pushes the growth rings.
struct Knot {
    x: f32,
    y: f32,
    rx: f32,
    ry: f32,
    push: f32,
    dead: bool,
    crack: f32,
}

/// A straight stroke (part of a scratch, scuff or arc) in tile units.
#[derive(Clone, Copy)]
struct Stroke {
    a: [f32; 2],
    b: [f32; 2],
    /// Half width, tile units.
    hw: f32,
    /// Intensity.
    k: f32,
    /// Position of `a` and `b` along the whole mark, 0..1.
    t0: f32,
    t1: f32,
    /// Which mark this stroke belongs to.
    id: u32,
}

/// Strokes binned on a `g` × `g` grid over the tile, so a pixel only tests nearby strokes.
/// Strokes wrap around the tile edges.
struct Strokes {
    g: usize,
    bins: Vec<Vec<u32>>,
    items: Vec<Stroke>,
    /// Extra band around every stroke, tile units (see [`Strokes::thin`]).
    pad: f32,
}

impl Strokes {
    fn new(g: usize, items: Vec<Stroke>) -> Self {
        Self::padded(g, items, 0.0)
    }

    /// For lines drawn with [`line_cov`] on an `n`-texel tile: also visits the texels next to
    /// strokes thinner than a texel.
    fn thin(g: usize, items: Vec<Stroke>, n: u32) -> Self {
        Self::padded(g, items, 1.5 / n as f32)
    }

    fn padded(g: usize, items: Vec<Stroke>, pad: f32) -> Self {
        let mut bins = vec![Vec::new(); g * g];
        let gi = g as i32;
        for (i, s) in items.iter().enumerate() {
            let w = s.hw + pad;
            let lo = [s.a[0].min(s.b[0]) - w, s.a[1].min(s.b[1]) - w];
            let hi = [s.a[0].max(s.b[0]) + w, s.a[1].max(s.b[1]) + w];
            let x0 = (lo[0] * g as f32).floor() as i32;
            let x1 = ((hi[0] * g as f32).floor() as i32).min(x0 + gi - 1);
            let y0 = (lo[1] * g as f32).floor() as i32;
            let y1 = ((hi[1] * g as f32).floor() as i32).min(y0 + gi - 1);
            for by in y0..=y1 {
                for bx in x0..=x1 {
                    let bin = by.rem_euclid(gi) as usize * g + bx.rem_euclid(gi) as usize;
                    bins[bin].push(i as u32);
                }
            }
        }
        Self {
            g,
            bins,
            items,
            pad,
        }
    }

    /// Call `f(stroke, signed distance, position along the mark 0..1)` for every stroke whose
    /// band (plus the padding) contains (u, v). Distances in tile units.
    fn visit(&self, u: f32, v: f32, mut f: impl FnMut(&Stroke, f32, f32)) {
        let bx = ((u * self.g as f32) as usize).min(self.g - 1);
        let by = ((v * self.g as f32) as usize).min(self.g - 1);
        for &i in &self.bins[by * self.g + bx] {
            let s = &self.items[i as usize];
            let mid = [(s.a[0] + s.b[0]) * 0.5, (s.a[1] + s.b[1]) * 0.5];
            let p = [mid[0] + wrap_d(u, mid[0]), mid[1] + wrap_d(v, mid[1])];
            let ab = [s.b[0] - s.a[0], s.b[1] - s.a[1]];
            let ap = [p[0] - s.a[0], p[1] - s.a[1]];
            let len2 = (ab[0] * ab[0] + ab[1] * ab[1]).max(1e-12);
            let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / len2).clamp(0.0, 1.0);
            let e = [ap[0] - ab[0] * t, ap[1] - ab[1] * t];
            let dist = (e[0] * e[0] + e[1] * e[1]).sqrt();
            if dist < s.hw + self.pad {
                let side = if ab[0] * ap[1] - ab[1] * ap[0] < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                f(s, dist * side, lerp(s.t0, s.t1, t));
            }
        }
    }
}

/// A cathedral figure of flat-sawn wood: the growth rings bulge by `amp` rings around a line
/// along U. The bulge rises sharply at one end (nested pointed arches), holds, then fades out
/// slowly so the lines open up again. Position and lengths in tile units, `sv` (the width
/// across) in meters.
struct Figure {
    u: f32,
    v: f32,
    rise: f32,
    hold: f32,
    fade: f32,
    sv: f32,
    amp: f32,
}

fn cathedral_figures(rng: &mut Rng, count: usize, amp: (f32, f32), sv: (f32, f32)) -> Vec<Figure> {
    (0..count)
        .map(|_| Figure {
            u: rng.next(),
            v: rng.next(),
            rise: rng.range(0.025, 0.06),
            hold: rng.range(0.05, 0.2),
            fade: rng.range(0.2, 0.3),
            sv: rng.range(sv.0, sv.1),
            amp: rng.range(amp.0, amp.1) * if rng.next() < 0.5 { 1.0 } else { -1.0 },
        })
        .collect()
}

/// Ring displacement (in rings) of all `figures` at (u, v). A figure spans less than a tile
/// along U, measured from its start `u`.
fn figure_push(figures: &[Figure], u: f32, v: f32, tile_m: f32) -> f32 {
    let mut push = 0.0;
    for f in figures {
        let dv = wrap_d(v, f.v) * tile_m;
        if dv.abs() > 3.0 * f.sv {
            continue;
        }
        let du = wrap_d(u, f.u + 0.45) + 0.45;
        let along = sstep(0.0, f.rise, du) * sstep(f.rise + f.hold + f.fade, f.rise + f.hold, du);
        push += f.amp * along * (-(dv / f.sv).powi(2)).exp();
    }
    push
}

/// Round marks (stains, drips) as zero-length strokes; radius and intensity ranges, radius in
/// meters. The band reaches 1.5 × the radius so shapes can be distorted: radius = `hw / 1.5`.
fn spots(
    rng: &mut Rng,
    count: usize,
    tile_m: f32,
    radius: (f32, f32),
    k: (f32, f32),
) -> Vec<Stroke> {
    (0..count as u32)
        .map(|i| {
            let p = [rng.next(), rng.next()];
            Stroke {
                a: p,
                b: p,
                hw: 1.5 * rng.range(radius.0, radius.1) / tile_m,
                k: rng.range(k.0, k.1),
                t0: 0.0,
                t1: 1.0,
                id: i,
            }
        })
        .collect()
}

/// Append an arc (center, radius, start angle, length, width in meters) as short strokes.
fn push_arc(out: &mut Vec<Stroke>, tile_m: f32, arc: [f32; 6], k: f32, id: u32) {
    let [cx, cy, radius, a0, len, width] = arc;
    let steps = (len / (radius * 0.06).clamp(0.02, 0.15)).ceil().max(2.0) as usize;
    let point = |s: usize| {
        let a = a0 + (s as f32 / steps as f32) * len / radius;
        [
            cx + radius / tile_m * a.cos(),
            cy + radius / tile_m * a.sin(),
        ]
    };
    for s in 0..steps {
        out.push(Stroke {
            a: point(s),
            b: point(s + 1),
            hw: 0.5 * width / tile_m,
            k,
            t0: s as f32 / steps as f32,
            t1: (s + 1) as f32 / steps as f32,
            id,
        });
    }
}

/// Curved tire scuffs, some as the pair of marks a vehicle's two wheels leave (1.55 m apart).
fn tire_marks(rng: &mut Rng, tile_m: f32, count: usize) -> Vec<Stroke> {
    let mut out = Vec::new();
    for i in 0..count as u32 {
        let (cx, cy) = (rng.next(), rng.next());
        let radius = rng.range(3.0, 9.0);
        let len = rng.range(1.2, 2.8);
        let a0 = rng.range(0.0, TAU);
        let width = rng.range(0.16, 0.22);
        let k = rng.range(0.25, 0.65);
        push_arc(&mut out, tile_m, [cx, cy, radius, a0, len, width], k, i * 2);
        if rng.next() < 0.6 {
            let r2 = radius + 1.55;
            let k2 = k * rng.range(0.6, 1.0);
            push_arc(
                &mut out,
                tile_m,
                [cx, cy, r2, a0, len * r2 / radius, width],
                k2,
                i * 2 + 1,
            );
        }
    }
    out
}

/// Arcs of a power trowel's blades: `count` arcs with radii and widths in meters.
fn swirl_marks(
    rng: &mut Rng,
    tile_m: f32,
    count: usize,
    radius: (f32, f32),
    width: (f32, f32),
) -> Vec<Stroke> {
    let mut out = Vec::new();
    for i in 0..count as u32 {
        let r = rng.range(radius.0, radius.1);
        let arc = [
            rng.next(),
            rng.next(),
            r,
            rng.range(0.0, TAU),
            r * rng.range(0.8, 2.2),
            rng.range(width.0, width.1),
        ];
        push_arc(&mut out, tile_m, arc, rng.range(0.3, 1.0), i);
    }
    out
}

/// Straight scratches: `count` strokes of `len` meters, 0.35 - 0.9 mm wide (scaled by
/// `width_scale`); 60 % of them roughly along U.
fn scratch_strokes(
    rng: &mut Rng,
    count: usize,
    tile_m: f32,
    len: (f32, f32),
    width_scale: f32,
) -> Vec<Stroke> {
    (0..count as u32)
        .map(|i| {
            let ang = if rng.next() < 0.6 {
                rng.range(-0.25, 0.25)
            } else {
                rng.range(0.0, TAU)
            };
            let l = rng.range(len.0, len.1) / tile_m;
            let (x, y) = (rng.next(), rng.next());
            Stroke {
                a: [x, y],
                b: [x + ang.cos() * l, y + ang.sin() * l],
                hw: rng.range(0.00035, 0.0009) * width_scale / tile_m,
                k: rng.range(0.3, 1.0),
                t0: 0.0,
                t1: 1.0,
                id: i,
            }
        })
        .collect()
}

/// Coverage of a line of half width `hw` at signed distance `d` (tile units) on an `n`-texel
/// tile. Lines thinner than a texel are drawn one texel wide and fainter, so they do not break
/// up into dots.
fn line_cov(hw: f32, d: f32, n: u32) -> f32 {
    let px = 1.0 / n as f32;
    let w = hw.max(0.9 * px);
    sat((w - d.abs()) / px + 0.5) * (hw / w)
}

// =============================================================================================
// Small math
// =============================================================================================

#[inline]
fn sat(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Hermite smoothstep; `e0 > e1` gives a falling edge.
#[inline]
fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = sat((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

#[inline]
fn scale3(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
fn mul3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

#[inline]
fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
