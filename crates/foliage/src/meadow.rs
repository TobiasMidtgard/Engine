//! Meadow communities: which grass grows where in one [`GrassField`], painted in big strokes
//! from what the ground is like, as Ghost of Tsushima paints its grass-type map ("as if by a
//! giant brush stroke") and Horizon Zero Dawn bundles plants into ecotopes
//! (engine-design/fields-and-forests-look.md, 4.1).
//!
//! A field grows up to sixteen kinds ([`GrassKind`]) in the same pass: its region's meadow
//! grass, grazed turf on trodden verges and steep thin soil, tall grass with seed heads in
//! moist hollows and along edges, dry grass on parched sunny ground, sedge on wet ground, clover,
//! bracken at forest edges, heather on dry rocky ground, fireweed where fire or felling opened
//! the ground, and the meadow, turf and dry grass again with their flowers (buttercups, daisies,
//! harebells, red clover, yarrow) in drifts. [`community`] weighs each kind for a [`Site`] and
//! lets a low-frequency noise per kind pick among the close ones, so kinds form swathes and
//! drifts a few tens of meters across rather than salt and pepper; [`palette`] gives the looks,
//! derived from the region's own grass so a savanna's or a tundra's communities keep its colors.

use engine_core::math::{noise::fbm2, smoothstep};
use engine_render::GrassStyle;

/// The kinds of grass a meadow grows, in palette order (`GrassField::kinds`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum GrassKind {
    /// The region's own meadow grass (the field's `style`).
    Meadow = 0,
    /// Short, dense grazed turf: paths' verges, around buildings, steep thin soil.
    Turf,
    /// Tall grass with seed heads: moist hollows, field edges, unmown patches.
    Tall,
    /// Dry and dead grass: parched, sunny ground.
    Dry,
    /// Sedge and rush: stiff, dark tussocks on wet ground.
    Sedge,
    /// Clover and forb rosettes: patches in the turf.
    Clover,
    /// Bracken fronds: forest edges, half shade.
    Bracken,
    /// Heather: low, woody, purple in late summer; dry, rocky ground.
    Heather,
    /// Meadow with buttercups.
    Buttercups,
    /// Meadow with oxeye daisies.
    Daisies,
    /// Turf with red clover heads.
    RedClover,
    /// Dry grass with yarrow.
    Yarrow,
    /// Meadow with harebells.
    Harebells,
    /// Fireweed spikes: burns and clearings.
    Fireweed,
}

/// How many kinds [`palette`] makes (with the field's own style).
pub const KINDS: usize = GrassKind::Fireweed as usize + 1;

/// What the ground is like at a point, each 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Site {
    /// Soil water: 0 parched .. 1 waterlogged (0.5 ordinary).
    pub water: f32,
    /// Shade from crowns overhead.
    pub shade: f32,
    /// Steepness: 0 flat .. 1 about 45 degrees.
    pub slope: f32,
    /// Facing the sun: -1 away (north in the north) .. 1 toward it.
    pub sunny: f32,
    /// Trodden, grazed or mown.
    pub wear: f32,
    /// A forest's edge or a field's margin.
    pub edge: f32,
    /// Opened by fire or felling: ash, rotting wood.
    pub opened: f32,
}

/// The kind of grass growing at `site`, world (or region) position `x`, `z` meters, for a
/// meadow seeded with `seed`.
pub fn community(site: &Site, x: f64, z: f64, seed: u64) -> GrassKind {
    use GrassKind::*;
    let s = site;
    let wet = smoothstep(0.62, 0.85, s.water);
    let moist = smoothstep(0.42, 0.65, s.water) * (1.0 - wet);
    let parched = 1.0 - smoothstep(0.18, 0.42, s.water);
    let steep = smoothstep(0.3, 0.7, s.slope);
    let open = 1.0 - smoothstep(0.35, 0.8, s.shade);
    let half_shade = smoothstep(0.15, 0.45, s.shade) * (1.0 - smoothstep(0.6, 0.9, s.shade));
    let sun = 0.5 + 0.5 * s.sunny.clamp(-1.0, 1.0);
    let base = [
        (Meadow, 1.0 * open * (1.0 - 0.6 * s.wear)),
        (Turf, 0.15 + 1.6 * s.wear + 0.7 * steep),
        (
            Tall,
            (0.9 * moist + 0.7 * s.edge) * (1.0 - s.wear) * (1.0 - 0.5 * steep),
        ),
        (Dry, 1.3 * parched * (0.5 + 0.5 * sun) + 0.3 * steep * sun),
        (Sedge, 2.2 * wet),
        (Clover, 0.45 * s.wear * (1.0 - parched)),
        (
            Bracken,
            (1.2 * s.edge + 0.6 * half_shade) * (1.0 - wet) * (1.0 - s.wear),
        ),
        (
            Heather,
            1.1 * parched * steep + 0.35 * parched * (1.0 - open),
        ),
        (Fireweed, 1.6 * s.opened * open * (1.0 - wet)),
    ];
    // Each kind's own brush: broad strokes tens of meters across, so close calls fall into
    // swathes instead of speckles.
    let brush = |k: GrassKind| {
        let salt = seed ^ (0x9e37_79b9_7f4a_7c15u64.wrapping_mul(k as u64 + 1));
        fbm2(x / 38.0, z / 38.0, salt, 3, 2.0, 0.5) as f32
    };
    let kind = base
        .iter()
        .map(|&(k, w)| (k, w * (1.0 + 0.9 * brush(k)).max(0.05)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(Meadow, |(k, _)| k);
    // Flowers in drifts a few meters across within their host.
    let drift = |salt: u64, scale: f64| fbm2(x / scale, z / scale, seed ^ salt, 3, 2.0, 0.5) as f32;
    match kind {
        Meadow => {
            let flowers = [
                (Buttercups, drift(0xb07, 11.0) + 0.1 * moist),
                (Daisies, drift(0xda1, 9.0) + 0.1 * sun),
                (Harebells, drift(0x4a3, 7.0) - 0.1 + 0.15 * parched),
            ];
            flowers
                .into_iter()
                .filter(|&(_, n)| n > 0.32)
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map_or(Meadow, |(k, _)| k)
        }
        Turf if drift(0xc10, 6.0) > 0.38 => RedClover,
        Dry if drift(0x7a4, 8.0) > 0.4 => Yarrow,
        // Fireweed spreads from seed blown into the open patches: stands, not a carpet.
        Fireweed if drift(0xf13, 12.0) < 0.1 => Tall,
        k => k,
    }
}

impl GrassKind {
    /// The grass a flowering kind grows in (itself for the others).
    pub fn host(self) -> Self {
        use GrassKind::*;
        match self {
            Buttercups | Daisies | Harebells => Meadow,
            RedClover => Turf,
            Yarrow => Dry,
            k => k,
        }
    }
}

/// How much of their flowering share flowers show on `day_of_year` (0..365): summer flowers
/// from late spring to late summer.
pub fn blooming(day_of_year: f32) -> f32 {
    smoothstep(120.0, 155.0, day_of_year) * (1.0 - smoothstep(230.0, 265.0, day_of_year))
}

/// Heather flowers late: August into September.
pub fn heather_blooming(day_of_year: f32) -> f32 {
    smoothstep(195.0, 220.0, day_of_year) * (1.0 - smoothstep(255.0, 280.0, day_of_year))
}

fn scaled(c: [f32; 3], f: f32) -> [f32; 3] {
    c.map(|v| v * f)
}

fn mixed(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// The looks of kinds 1.. (`GrassField::styles`) for a meadow whose own grass is `meadow`
/// (kind 0, `GrassField::style`), on `day_of_year`.
pub fn palette(meadow: &GrassStyle, day_of_year: f32) -> Vec<GrassStyle> {
    use GrassKind::*;
    let bloom = blooming(day_of_year);
    let (lush, dry) = (meadow.lush_color, meadow.dry_color);
    let h = meadow.height.max(0.05);
    let flowering = |base: GrassStyle, share: f32, color: [f32; 3], size: f32| GrassStyle {
        bloom: share * bloom,
        bloom_color: color,
        bloom_size: size * 1.3,
        bloom_length: 0.0,
        ..base
    };
    // (No kind outnumbers the meadow's own blades: the field's most sets how many threads
    // every tile starts.)
    let turf = GrassStyle {
        blades_per_m2: meadow.blades_per_m2,
        height: (h * 0.3).clamp(0.05, 0.16),
        height_variation: 0.35,
        width: meadow.width * 0.75,
        bend: 0.25,
        lush_color: scaled(lush, 0.85),
        clump_size: 0.35,
        clump_pull: 0.1,
        clump_height: 0.2,
        ..*meadow
    };
    let tall = GrassStyle {
        blades_per_m2: meadow.blades_per_m2 * 0.6,
        height: (h * 2.6).clamp(0.6, 1.2),
        height_variation: 0.3,
        width: meadow.width * 1.15,
        bend: 0.55,
        droop: 0.45,
        lush_color: scaled(mixed(lush, [0.07, 0.15, 0.06], 0.4), 0.95),
        clump_size: 0.9,
        clump_pull: 0.3,
        // Seed heads: pale spikes and plumes (timothy, foxtail, cocksfoot) standing above
        // the blades, from early summer, bleaching to straw.
        bloom: 0.18
            * smoothstep(140.0, 170.0, day_of_year)
            * (1.0 - smoothstep(290.0, 330.0, day_of_year)),
        bloom_color: mixed(dry, [0.42, 0.44, 0.24], 0.5),
        bloom_size: 0.016,
        bloom_length: 0.16,
        ..*meadow
    };
    let parched = GrassStyle {
        blades_per_m2: meadow.blades_per_m2 * 0.8,
        height: h * 1.1,
        height_variation: 0.5,
        lush_color: mixed(lush, dry, 0.55),
        dry_color: scaled(dry, 1.1),
        bend: 0.5,
        // Dry grass heads early and sparsely (bent and fescue panicles), straw coloured.
        bloom: 0.12
            * smoothstep(130.0, 160.0, day_of_year)
            * (1.0 - smoothstep(280.0, 320.0, day_of_year)),
        bloom_color: mixed(dry, [0.55, 0.48, 0.32], 0.5),
        bloom_size: 0.016,
        bloom_length: 0.12,
        ..*meadow
    };
    let mut kinds = vec![GrassStyle::default(); KINDS - 1];
    let mut set = |k: GrassKind, s: GrassStyle| kinds[k as usize - 1] = s;
    set(Turf, turf);
    set(Tall, tall);
    set(Dry, parched);
    set(
        Sedge,
        GrassStyle {
            blades_per_m2: meadow.blades_per_m2 * 0.9,
            height: (h * 1.8).clamp(0.4, 0.9),
            height_variation: 0.3,
            width: meadow.width * 0.85,
            bend: 0.15,
            droop: 0.12,
            lush_color: [0.045, 0.1, 0.05],
            dry_color: [0.22, 0.2, 0.1],
            clump_size: 0.6,
            clump_pull: 0.5,
            clump_fan: 0.7,
            gloss: 0.55,
            ..*meadow
        },
    );
    set(
        Clover,
        GrassStyle {
            blades_per_m2: meadow.blades_per_m2 * 0.55,
            height: 0.11,
            height_variation: 0.3,
            width: 0.028,
            bend: 0.3,
            lush_color: [0.055, 0.14, 0.04],
            dry_color: [0.2, 0.18, 0.08],
            clump_size: 0.3,
            clump_pull: 0.55,
            clump_fan: 1.0,
            droop: 0.5,
            gloss: 0.4,
            tip_tint: [1.0, 1.0, 1.0],
            leaf: 1.0,
            stalk: 0.5,
            ..*meadow
        },
    );
    set(
        Bracken,
        GrassStyle {
            blades_per_m2: 14.0,
            // Bracken's fronds are cut into pinnae too, coarser.
            pinnae: 14.0,
            height: 0.85,
            height_variation: 0.35,
            width: 0.32,
            bend: 0.6,
            lush_color: [0.06, 0.15, 0.035],
            // Bracken turns rust brown in autumn.
            dry_color: [0.3, 0.13, 0.04],
            clump_size: 1.2,
            clump_pull: 0.2,
            clump_fan: 1.0,
            clump_height: 0.3,
            droop: 0.55,
            gloss: 0.3,
            tip_tint: [1.1, 1.05, 0.8],
            leaf: 1.0,
            stalk: 0.4,
            ..*meadow
        },
    );
    set(
        Heather,
        GrassStyle {
            blades_per_m2: meadow.blades_per_m2,
            height: 0.28,
            height_variation: 0.35,
            width: meadow.width * 0.6,
            bend: 0.2,
            droop: 0.1,
            lush_color: [0.035, 0.06, 0.03],
            dry_color: [0.14, 0.1, 0.06],
            clump_size: 0.45,
            clump_pull: 0.55,
            clump_height: 0.4,
            gloss: 0.15,
            bloom: 0.45 * heather_blooming(day_of_year),
            bloom_color: [0.3, 0.1, 0.3],
            bloom_size: 0.012,
            ..*meadow
        },
    );
    set(
        Buttercups,
        flowering(*meadow, 0.08, [0.72, 0.55, 0.02], 0.022),
    );
    set(Daisies, flowering(*meadow, 0.07, [0.78, 0.78, 0.7], 0.032));
    set(RedClover, flowering(turf, 0.07, [0.5, 0.12, 0.22], 0.02));
    set(Yarrow, flowering(parched, 0.05, [0.72, 0.7, 0.62], 0.04));
    set(Harebells, flowering(*meadow, 0.05, [0.2, 0.2, 0.55], 0.016));
    set(
        Fireweed,
        flowering(
            GrassStyle {
                height: (h * 3.0).clamp(0.8, 1.3),
                bend: 0.25,
                ..tall
            },
            0.14,
            [0.42, 0.1, 0.28],
            0.03,
        ),
    );
    // Fireweed flowers up a tall spike.
    kinds[Fireweed as usize - 1].bloom_length = 0.22;
    kinds
}

/// Kinds for a grid of `resolution` points over `size` meters (row by row along x, the first
/// at the -x -z corner, as `GrassField`'s maps), from the site at each point (`None`: the
/// field's own grass). `origin` is the world position of the -x -z corner, so the strokes
/// stay put when a field is rebuilt or moved.
pub fn paint_kinds(
    resolution: [u32; 2],
    size: [f32; 2],
    origin: [f64; 2],
    seed: u64,
    mut site: impl FnMut(f32, f32) -> Option<Site>,
) -> Vec<u8> {
    let [rx, rz] = resolution.map(|r| r.max(2));
    let mut out = Vec::with_capacity((rx * rz) as usize);
    for j in 0..rz {
        for i in 0..rx {
            let x = i as f32 / (rx - 1) as f32 * size[0];
            let z = j as f32 / (rz - 1) as f32 * size[1];
            let kind = site(x, z).map_or(GrassKind::Meadow, |s| {
                community(&s, origin[0] + x as f64, origin[1] + z as f64, seed)
            });
            out.push(kind as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use GrassKind::*;

    fn census(site: Site) -> std::collections::HashMap<GrassKind, usize> {
        let mut out = std::collections::HashMap::new();
        for j in 0..60 {
            for i in 0..60 {
                let k = community(&site, i as f64 * 3.0, j as f64 * 3.0, 7);
                *out.entry(k).or_default() += 1;
            }
        }
        out
    }

    fn share(c: &std::collections::HashMap<GrassKind, usize>, kinds: &[GrassKind]) -> f32 {
        kinds
            .iter()
            .map(|k| c.get(k).copied().unwrap_or(0))
            .sum::<usize>() as f32
            / 3600.0
    }

    #[test]
    fn each_kind_holds_the_ground_that_suits_it() {
        let open = Site {
            water: 0.5,
            ..Default::default()
        };
        let meadow = census(open);
        assert!(
            share(&meadow, &[Meadow, Buttercups, Daisies, Harebells]) > 0.5,
            "{meadow:?}"
        );
        // Flowers in drifts, not everywhere.
        let flowers = share(&meadow, &[Buttercups, Daisies, Harebells]);
        assert!((0.05..0.45).contains(&flowers), "{flowers}");
        let wet = census(Site {
            water: 0.92,
            ..open
        });
        assert!(share(&wet, &[Sedge]) > 0.6, "{wet:?}");
        let path = census(Site { wear: 0.9, ..open });
        assert!(share(&path, &[Turf, RedClover, Clover]) > 0.7, "{path:?}");
        let parched = census(Site {
            water: 0.1,
            sunny: 1.0,
            ..open
        });
        assert!(
            share(&parched, &[Dry, Yarrow, Heather]) > 0.6,
            "{parched:?}"
        );
        let edge = census(Site {
            edge: 1.0,
            shade: 0.35,
            ..open
        });
        assert!(share(&edge, &[Bracken, Tall]) > 0.5, "{edge:?}");
        let burn = census(Site {
            opened: 1.0,
            ..open
        });
        // Fireweed in stands among tall grass, not a carpet.
        assert!(share(&burn, &[Fireweed]) > 0.2, "{burn:?}");
        assert!(share(&burn, &[Fireweed, Tall]) > 0.8, "{burn:?}");
    }

    /// Kinds form swathes: neighbours 3 m apart mostly agree.
    #[test]
    fn kinds_come_in_swathes_not_speckles() {
        let site = Site {
            water: 0.55,
            edge: 0.3,
            ..Default::default()
        };
        let (mut same, mut flowers, mut all) = (0, 0, 0);
        for j in 0..80 {
            for i in 0..80 {
                let (x, z) = (i as f64 * 3.0, j as f64 * 3.0);
                let a = community(&site, x, z, 3);
                let b = community(&site, x + 3.0, z, 3);
                same += usize::from(a.host() == b.host());
                flowers += usize::from(a == b);
                all += 1;
            }
        }
        // The grasses in swathes; their flowers in smaller drifts.
        assert!(same as f32 / all as f32 > 0.85, "{same} of {all}");
        assert!(flowers as f32 / all as f32 > 0.65, "{flowers} of {all}");
    }

    #[test]
    fn the_palette_fills_every_kind_and_flowers_only_in_season() {
        let base = GrassStyle::meadow();
        let summer = palette(&base, 180.0);
        assert_eq!(summer.len(), KINDS - 1);
        assert!(summer.len() < engine_render::MAX_GRASS_KINDS);
        let at = |p: &[GrassStyle], k: GrassKind| p[k as usize - 1];
        assert!(at(&summer, Buttercups).bloom > 0.0);
        assert!(at(&summer, Tall).height > 2.0 * at(&summer, Turf).height);
        let winter = palette(&base, 20.0);
        assert_eq!(at(&winter, Buttercups).bloom, 0.0);
        assert_eq!(at(&winter, Heather).bloom, 0.0);
        assert!(at(&palette(&base, 225.0), Heather).bloom > 0.0);
    }
}
