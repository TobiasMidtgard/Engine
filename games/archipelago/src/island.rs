//! The archipelago: where the islands are and the shape of their ground and sea bed.
//!
//! One pure height function describes every island and the sea floor between them. The
//! terrain module draws it (an overlay on a flat world whose own ground is all sea), the water
//! module floods it, and the build grid, carts, ships and the camera all read the same numbers,
//! so what a building stands on is what is drawn.

use engine::core::math::noise::{fbm2, perlin2};
use engine::prelude::*;

use crate::defs::Fertility;
use crate::globe::{self, Biome};

/// One island of the map.
#[derive(Clone, Debug)]
pub struct Island {
    pub name: &'static str,
    /// World X, Z of its middle, meters.
    pub center: DVec2,
    /// Rough radius to the coast, meters.
    pub radius: f64,
    /// Tallest hills above the plateau, meters.
    pub hills_m: f64,
    /// Which way its cliffs face (radians about Y, 0 = +X) and how much of the coast they take.
    pub cliff_dir: f64,
    pub cliff_share: f64,
    pub seed: u64,
    pub fertility: Fertility,
    /// Forest share of the land, 0..1.
    pub woods: f64,
    /// The island (by index) its settlement's harbour faces, if it has one: the coast there is
    /// kept gentle and the land behind it flat enough for a town.
    pub settle_toward: Option<usize>,
    /// How far that calm ground reaches from the harbour, meters.
    pub settle_m: f64,
    /// How much coarser its coast and hills are than a small island's (1 for an island; a
    /// continent's bays and ranges are a few times larger).
    pub grain: f64,
}

impl Island {
    /// The region (sheet of the map) it lies in.
    pub fn region(&self) -> usize {
        globe::region_of(self.center.x, self.center.y)
    }

    pub fn biome(&self) -> Biome {
        globe::regions()[self.region()].biome
    }

    /// 1 for an island, smaller for a continent: its beaches, banks and shelves keep an
    /// island's width instead of growing with the land.
    fn fine(&self) -> f64 {
        (800.0 / self.radius).min(1.0)
    }
}

/// The map. The home island is first and sits at the world origin.
pub fn islands() -> &'static [Island] {
    static ISLANDS: std::sync::OnceLock<Vec<Island>> = std::sync::OnceLock::new();
    ISLANDS.get_or_init(|| {
        let mut v = vec![
            Island {
                name: "Haverholm",
                center: DVec2::new(0.0, 0.0),
                radius: 820.0,
                hills_m: 70.0,
                cliff_dir: 1.9,
                cliff_share: 0.3,
                seed: 11,
                fertility: Fertility {
                    wool: true,
                    clay: true,
                    ..Default::default()
                },
                woods: 0.42,
                settle_toward: Some(1),
                settle_m: 430.0,
                grain: 1.0,
            },
            Island {
                name: "Brannsey",
                center: DVec2::new(2350.0, -1250.0),
                radius: 560.0,
                hills_m: 34.0,
                cliff_dir: 0.6,
                cliff_share: 0.2,
                seed: 23,
                fertility: Fertility {
                    grain: true,
                    hops: true,
                    ..Default::default()
                },
                woods: 0.25,
                settle_toward: Some(0),
                settle_m: 300.0,
                grain: 1.0,
            },
            Island {
                name: "Skerryvale",
                center: DVec2::new(-2150.0, -1500.0),
                radius: 420.0,
                hills_m: 62.0,
                cliff_dir: 4.0,
                cliff_share: 0.35,
                seed: 37,
                // (Its sheltered flats make salt, for the Burghers' salt fish.)
                fertility: Fertility {
                    hops: true,
                    clay: true,
                    salt: true,
                    ..Default::default()
                },
                woods: 0.5,
                settle_toward: Some(0),
                settle_m: 240.0,
                grain: 1.0,
            },
            Island {
                name: "Northmoor",
                center: DVec2::new(-1300.0, 2250.0),
                radius: 640.0,
                hills_m: 125.0,
                cliff_dir: 2.6,
                cliff_share: 0.4,
                seed: 41,
                fertility: Fertility {
                    grain: true,
                    wool: true,
                    ore: true,
                    ..Default::default()
                },
                woods: 0.55,
                settle_toward: Some(0),
                settle_m: 300.0,
                grain: 1.0,
            },
            Island {
                name: "Little Tarn",
                center: DVec2::new(1750.0, 1650.0),
                radius: 260.0,
                hills_m: 58.0,
                cliff_dir: 5.2,
                cliff_share: 0.6,
                seed: 53,
                // (Deep woods with game in them: trappers' furs for the Burghers' coats.)
                fertility: Fertility {
                    furs: true,
                    ..Default::default()
                },
                woods: 0.7,
                settle_toward: None,
                settle_m: 0.0,
                grain: 1.0,
            },
        ];
        v.extend(far_lands());
        v
    })
}

/// The continents and islands of the other regions, each on its own sheet of the map.
fn far_lands() -> Vec<Island> {
    let land = |region: usize,
                name: &'static str,
                at: (f64, f64),
                radius: f64,
                hills_m: f64,
                cliffs: (f64, f64),
                seed: u64,
                woods: f64,
                fertility: Fertility,
                settle: Option<(usize, f64)>| {
        let o = globe::sheet_origin(region);
        Island {
            name,
            center: o + DVec2::new(at.0, at.1),
            radius,
            hills_m,
            cliff_dir: cliffs.0,
            cliff_share: cliffs.1,
            seed,
            fertility,
            woods,
            settle_toward: settle.map(|s| s.0),
            settle_m: settle.map_or(0.0, |s| s.1),
            grain: (radius / 820.0).max(1.0).powf(0.75),
        }
    };
    let none = Fertility::default();
    vec![
        // --- Sarrowmere: a jungle continent and its keys (islands 5..=8).
        land(
            1,
            "Sarrowmere",
            (-900.0, 500.0),
            4200.0,
            330.0,
            (3.6, 0.25),
            101,
            0.78,
            // (Highland pasture above the jungle: a house can clothe its first town here.)
            Fertility {
                plantains: true,
                cotton: true,
                clay: true,
                grain: true,
                cane: true,
                wool: true,
                cacao: true,
                ..none
            },
            Some((6, 520.0)),
        ),
        land(
            1,
            "Palm Key",
            (5600.0, -900.0),
            520.0,
            40.0,
            (0.4, 0.15),
            103,
            0.6,
            Fertility {
                plantains: true,
                cotton: true,
                hops: true,
                cane: true,
                cacao: true,
                ..none
            },
            Some((5, 260.0)),
        ),
        land(
            1,
            "Heron Cay",
            (4300.0, 4900.0),
            380.0,
            26.0,
            (5.0, 0.1),
            107,
            0.5,
            Fertility {
                plantains: true,
                coffee: true,
                ..none
            },
            None,
        ),
        land(
            1,
            "Tamarin",
            (-6400.0, -4300.0),
            700.0,
            120.0,
            (2.2, 0.4),
            109,
            0.8,
            Fertility {
                plantains: true,
                cotton: true,
                clay: true,
                cane: true,
                coffee: true,
                ..none
            },
            None,
        ),
        // --- The Amber Reach: dry hills and thorn savanna (islands 9..=11).
        land(
            2,
            "Amberland",
            (600.0, -300.0),
            4600.0,
            380.0,
            (1.2, 0.45),
            211,
            0.14,
            Fertility {
                goats: true,
                tea: true,
                wool: true,
                clay: true,
                dates: true,
                silk: true,
                ..none
            },
            Some((10, 560.0)),
        ),
        land(
            2,
            "Saltholm",
            (-6300.0, 2400.0),
            640.0,
            45.0,
            (4.4, 0.3),
            223,
            0.08,
            Fertility {
                goats: true,
                wool: true,
                salt: true,
                dates: true,
                gold: true,
                ..none
            },
            Some((9, 280.0)),
        ),
        land(
            2,
            "The Kettle",
            (5200.0, 5600.0),
            430.0,
            150.0,
            (0.0, 0.8),
            227,
            0.1,
            Fertility {
                goats: true,
                tea: true,
                salt: true,
                ore: true,
                ..none
            },
            None,
        ),
        // --- Frostholm: ice and dark spruce (islands 12..=15).
        land(
            3,
            "Frostholm",
            (300.0, 500.0),
            3700.0,
            460.0,
            (5.6, 0.5),
            307,
            0.32,
            // (Hardy barley on the southern slopes: bread for a town of its own.)
            Fertility {
                wool: true,
                grain: true,
                furs: true,
                oil: true,
                ..none
            },
            Some((13, 480.0)),
        ),
        land(
            3,
            "Whalers' Rock",
            (5300.0, -1500.0),
            460.0,
            60.0,
            (2.8, 0.5),
            311,
            0.2,
            // (Brine boiled over the try-works fires: the north's only salt.)
            Fertility {
                oil: true,
                salt: true,
                ..none
            },
            Some((12, 230.0)),
        ),
        land(
            3,
            "Skarn",
            (-4700.0, -4400.0),
            820.0,
            240.0,
            (1.0, 0.6),
            313,
            0.25,
            Fertility {
                furs: true,
                oil: true,
                ore: true,
                ..none
            },
            None,
        ),
        land(
            3,
            "Little Skarn",
            (-5800.0, 3600.0),
            300.0,
            70.0,
            (3.3, 0.7),
            317,
            0.1,
            Fertility {
                oil: true,
                gold: true,
                ..none
            },
            None,
        ),
    ]
}

/// The sea floor between a region's islands, meters.
pub const OPEN_SEA_M: f64 = -42.0;
/// The ocean floor between the regions, meters.
pub const OCEAN_M: f64 = -90.0;
/// Lowest sea bed and highest ground the map holds, meters.
pub const HEIGHT_RANGE_M: (f64, f64) = (-110.0, 700.0);

fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Where `isle`'s settlement meets the sea (roughly: the coast toward the island it trades
/// with), if it has one.
pub fn settle_spot(isle: &Island) -> Option<DVec2> {
    let t = isle.settle_toward?;
    let dir = (islands()[t].center - isle.center).normalize();
    Some(isle.center + dir * isle.radius * 0.92)
}

/// Where the shore lies on the way out from the land's middle through its settlement spot:
/// the harbour of its first town goes here.
pub fn settle_coast(isle: &Island) -> Option<DVec2> {
    let spot = settle_spot(isle)?;
    let dir = (spot - isle.center).normalize();
    let mut p = spot - dir * 600.0;
    for _ in 0..400 {
        if height(p.x, p.y) < 0.5 {
            break;
        }
        p += dir * 5.0;
    }
    Some(p)
}

/// Where settlers sailing to `isle` best come ashore for its settlement: the nearest shore
/// reached from the settlement spot over low ground (nothing above 10 m on the way) with
/// water a ship can lie in beyond it; [`settle_coast`] when no such shore is in reach.
pub fn landing_coast(isle: &Island) -> Option<DVec2> {
    let spot = settle_spot(isle)?;
    let mut best: Option<(f64, DVec2)> = None;
    for k in 0..72 {
        let a = k as f64 * std::f64::consts::TAU / 72.0;
        let d = DVec2::new(a.cos(), a.sin());
        let mut walked = 0.0;
        let shore = loop {
            if walked > 2_500.0 {
                break None;
            }
            let p = spot + d * walked;
            let h = height(p.x, p.y);
            if h < 0.5 {
                let off = p + d * 80.0;
                break (height(off.x, off.y) < -2.0).then_some(p);
            }
            if h > 10.0 {
                break None;
            }
            walked += 10.0;
        };
        if let Some(p) = shore
            && best.is_none_or(|b| walked < b.0)
        {
            best = Some((walked, p));
        }
    }
    best.map(|b| b.1).or_else(|| settle_coast(isle))
}

/// 1 on the calm ground around `isle`'s settlement, 0 beyond `reach` times its calm radius.
fn calm(isle: &Island, x: f64, z: f64, reach: f64) -> f64 {
    let Some(spot) = settle_spot(isle) else {
        return 0.0;
    };
    let d = (DVec2::new(x, z) - spot).length();
    let r = isle.settle_m * reach;
    smooth(r, r * 0.55, d)
}

/// Big lobes of the coastline at bearing `a`: peninsulas where positive, bights where negative.
fn lobes(isle: &Island, a: f64) -> f64 {
    let mut sum = 0.0;
    for k in 2..=5u64 {
        let phase = ((isle.seed * 7919 + k * 104_729) % 10_007) as f64 * 0.618_034;
        sum += (0.26 / k as f64) * (k as f64 * a + phase).cos();
    }
    sum
}

/// "Inland-ness" of a point for one island: 0 at the coast, 1 at the middle, negative at sea
/// (−1 about one radius out).
fn inland(isle: &Island, x: f64, z: f64) -> f64 {
    let d = DVec2::new(x, z) - isle.center;
    let r = d.length() / isle.radius;
    if r > 2.6 {
        return 1.0 - r;
    }
    let s = isle.seed;
    let g = isle.grain;
    let wild = 1.0 - calm(isle, x, z, 1.0);
    // Coastline: lobes (peninsulas and bights), then bays and headlands from warped noise.
    // A continent's bays are wider than an island's, and shallower for its size.
    let lobe = lobes(isle, d.y.atan2(d.x)) * wild;
    let amp = g.powf(-0.6);
    let mut warp = fbm2(x / (520.0 * g), z / (520.0 * g), s, 3, 2.0, 0.5) as f64 * 0.34 * amp
        + fbm2(x / (140.0 * g), z / (140.0 * g), s + 1, 3, 2.1, 0.5) as f64 * 0.07 * amp;
    if g > 1.3 {
        // And still the small coves and points an island has.
        warp += fbm2(x / 170.0, z / 170.0, s + 13, 3, 2.1, 0.5) as f64 * 0.07 * isle.fine();
    }
    // Coves: narrow inlets where a noise field crosses zero, only in the outer band and only
    // on some stretches of coast.
    let cw = 340.0 * g.sqrt();
    let n = fbm2(x / cw, z / cw, s + 11, 3, 2.0, 0.5) as f64;
    let channel = (1.0 - n.abs() * 4.0).max(0.0).powi(2);
    let stretch = smooth(
        -0.15,
        0.15,
        perlin2(x / (800.0 * g), z / (800.0 * g), s + 12) as f64,
    );
    let band = smooth(0.45, 0.8, r) * smooth(1.25, 1.0, r);
    let cove = 0.4 * g.powf(-0.5) * channel * stretch * band * wild;
    1.0 - r * (1.0 - lobe) - warp - cove
}

/// Cliff strength 0..1 along this island's coast at `(x, z)`: one long run of cliffs facing
/// `cliff_dir`, and the tips of headlands, but never at the settlement.
fn cliff(isle: &Island, x: f64, z: f64) -> f64 {
    let d = DVec2::new(x, z) - isle.center;
    let a = d.y.atan2(d.x);
    let mut off = (a - isle.cliff_dir).rem_euclid(std::f64::consts::TAU);
    if off > std::f64::consts::PI {
        off -= std::f64::consts::TAU;
    }
    let half = isle.cliff_share * std::f64::consts::PI;
    let ww = 260.0 * isle.grain;
    let wobble = perlin2(x / ww, z / ww, isle.seed + 5) as f64 * 0.35;
    let run = smooth(half, half * 0.55, off.abs() + wobble * half);
    let headland = smooth(0.06, 0.16, lobes(isle, a));
    run.max(headland * 0.85) * (1.0 - calm(isle, x, z, 1.25))
}

/// Height of one island's ground (or its sea bed), meters.
fn island_height(isle: &Island, x: f64, z: f64) -> f64 {
    let t = inland(isle, x, z);
    let s = isle.seed;
    let c = cliff(isle, x, z);
    // Widths along the coast are an island's, however large the land (`t` is in radii).
    let k = isle.fine();
    let g = isle.grain;
    if t < 0.0 {
        // Sea bed: a shelf, then down to the open sea.
        // (The first meter and a half goes in eight meters of wet sand, not in a step: the
        // ground would show it as a ledge of rock along every beach.)
        let lip = 1.5 * smooth(0.0, 8.0 / isle.radius, -t);
        let shelf = -lip - 9.0 * smooth(0.0, 0.08 * k, -t);
        let deep = open_sea(x, z);
        // Under cliffs the bed drops away fast; elsewhere a sandy shelf runs out.
        let drop = smooth(0.0, (0.55 - 0.45 * c) * k.sqrt(), -t);
        return shelf + (deep - shelf) * drop;
    }
    // Beach then a low bank up to the plateau; cliffs instead rise sheer from the water.
    let beach_w = (0.07 * (1.0 - c) + 0.006 * c) * k;
    let beach = 1.6 * smooth(0.0, beach_w * 0.6, t);
    // Cliff tops are high downs that settle back into the rolling land further in.
    let bank_top = 5.0 + (24.0 + 22.0 * (g - 1.0)) * c * smooth(0.4 * k.sqrt(), 0.08 * k, t);
    let bank = bank_top * smooth(beach_w * 0.5, beach_w + (0.05 * (1.0 - c) + 0.01) * k, t);
    // Rolling land: what towns are built on; calmer near the settlement.
    let town = calm(isle, x, z, 1.35);
    let roll = fbm2(x / 380.0, z / 380.0, s + 2, 3, 2.0, 0.5) as f64 * (4.0 - 2.4 * town);
    let land = smooth(0.02 * k, 0.12 * k, t);
    // Hills where the hill mask says so, ridged, and a high middle on every island.
    let mask = smooth(
        0.0,
        0.4,
        fbm2(x / (700.0 * g), z / (700.0 * g), s + 3, 2, 2.0, 0.5) as f64 + 0.2,
    );
    let ridge = {
        let n = fbm2(x / (260.0 * g), z / (260.0 * g), s + 4, 4, 2.05, 0.5) as f64;
        (1.0 - n.abs() * 1.6).max(0.0).powi(2)
    };
    // The high middle follows plain distance (warped), not the lobed coast, so it has no
    // creases running out to the headlands.
    let rr = (DVec2::new(x, z) - isle.center).length() / isle.radius
        + fbm2(x / (450.0 * g), z / (450.0 * g), s + 6, 3, 2.0, 0.5) as f64 * 0.3;
    let crown = smooth(0.8, 0.1, rr) * (0.6 + 0.4 * ridge);
    // A continent keeps wide lowlands behind its coasts: its hills start further in.
    let rise = smooth(0.15 * k.sqrt(), 0.6 * k.powf(0.25), t);
    let mut hills = isle.hills_m
        * (mask * rise * (0.3 + 0.7 * ridge) * 0.75 + 0.45 * crown * rise.max(k))
        * (1.0 - town);
    if g > 1.3 {
        // Spurs and gullies an island's hills are too small to carry.
        let fine = fbm2(x / 190.0, z / 190.0, s + 14, 4, 2.1, 0.5) as f64;
        hills *= 1.0 + 0.22 * fine;
        hills += 9.0 * fine * rise;
    }
    bank.max(beach) + land * (roll + hills)
}

/// The sea floor away from the islands.
fn open_sea(x: f64, z: f64) -> f64 {
    OPEN_SEA_M + 3.0 * perlin2(x / 1500.0, z / 1500.0, 99) as f64
}

/// Height of the ground or sea bed at map point `(x, z)`, meters (sea level 0): the lands of
/// the sheet the point is on. Out past the region's reach the bed falls to the ocean floor.
pub fn height(x: f64, z: f64) -> f64 {
    let share = globe::reach_share(x, z);
    if share > 1.02 {
        return OCEAN_M;
    }
    let mut h = open_sea(x, z);
    for isle in islands() {
        let d2 = (DVec2::new(x, z) - isle.center).length_squared();
        if d2 > (isle.radius * 3.2).powi(2) {
            continue;
        }
        h = h.max(island_height(isle, x, z));
    }
    if share > 0.86 {
        let out = smooth(0.86, 1.02, share);
        h = h.min(0.0) * (1.0 - out) + OCEAN_M * out + h.max(0.0) * (1.0 - out);
    }
    h
}

/// Height of the ground or sea bed under planet-local direction `d`, meters: what the
/// terrain draws.
pub fn ground(d: DVec3) -> f64 {
    let m = globe::map_of(d);
    height(m.x, m.z)
}

/// Height at a map point that may lie beyond its own sheet's region (a ship in mid-ocean
/// charted from the port it left): the ground of whichever region is nearest there.
pub fn true_height(x: f64, z: f64) -> f64 {
    if globe::reach_share(x, z) < 1.0 {
        return height(x, z);
    }
    ground(globe::dir(x, z))
}

/// The island whose land is at `(x, z)`, if any.
pub fn island_at(x: f64, z: f64) -> Option<usize> {
    islands().iter().position(|isle| {
        (DVec2::new(x, z) - isle.center).length() < isle.radius * 1.6 && inland(isle, x, z) > 0.0
    })
}

/// The island nearest `(x, z)` (on land or off its coast).
pub fn nearest_island(x: f64, z: f64) -> usize {
    let p = DVec2::new(x, z);
    islands()
        .iter()
        .enumerate()
        .min_by(|a, b| {
            let da = (p - a.1.center).length() / a.1.radius;
            let db = (p - b.1.center).length() / b.1.radius;
            da.total_cmp(&db)
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Ground slope at `(x, z)`: the rise over `step` meters (largest of the two axes).
pub fn slope(x: f64, z: f64, step: f64) -> f64 {
    let h = height(x, z);
    let dx = (height(x + step, z) - h).abs();
    let dz = (height(x, z + step) - h).abs();
    dx.max(dz) / step
}

/// Roughly where the home town grows: on Haverholm's coast facing Brannsey. The woods keep
/// clear of it.
pub fn home_town_spot() -> DVec2 {
    let isles = islands();
    let d = (isles[1].center - isles[0].center).normalize();
    isles[0].center + d * isles[0].radius * 0.78
}

/// Forest density 0..1 at `(x, z)`: where woods grow untouched.
pub fn woods(x: f64, z: f64) -> f64 {
    let Some(i) = island_at(x, z) else {
        return 0.0;
    };
    let isle = &islands()[i];
    let t = inland(isle, x, z);
    let h = height(x, z);
    if h < 2.5 || t < 0.06 * isle.fine() {
        return 0.0;
    }
    let g = isle.grain.sqrt();
    let n = fbm2(x / (330.0 * g), z / (330.0 * g), isle.seed + 7, 3, 2.0, 0.5) as f64
        + fbm2(x / 70.0, z / 70.0, isle.seed + 8, 2, 2.0, 0.5) as f64 * 0.25;
    // Hills are wooded more often than flats; the home island's middle is kept open for the town.
    let hilly = smooth(8.0, 25.0, h) * 0.35;
    let open_town = if i == 0 {
        smooth(300.0, 170.0, (DVec2::new(x, z) - home_town_spot()).length())
    } else {
        calm(isle, x, z, 0.8) * 0.85
    };
    // Trees give out with height: soonest in the cold, and the dry hills are bare.
    let line = match isle.biome() {
        Biome::Temperate => 300.0,
        Biome::Tropical => 360.0,
        Biome::Arid => 140.0,
        Biome::Arctic => 110.0,
    };
    let tree_line = smooth(line, line * 0.7, h);
    smooth(0.55 - isle.woods, 0.75 - isle.woods, n + hilly) * (1.0 - open_town) * tree_line
}

/// Ground layer and tint for the terrain's paint: sand on beaches, rock on cliffs, the
/// region's own ground on the plateau (meadow, jungle floor, dry savanna, tundra and snow),
/// and a dark floor under the woods so they read from far above.
pub fn paint(x: f64, z: f64) -> Option<engine_terrain::OverlayPaint> {
    use engine_terrain::OverlayPaint;
    let h = height(x, z);
    let biome = globe::region_at(x, z).biome;
    let sand_tint = match biome {
        Biome::Tropical => Some(([0.80, 0.76, 0.64], 0.45)),
        Biome::Arid => Some(([0.62, 0.42, 0.24], 0.4)),
        Biome::Arctic => Some(([0.30, 0.30, 0.31], 0.5)),
        Biome::Temperate => None,
    };
    let sand = |w: f32| {
        let p = OverlayPaint::layer("sand", w);
        match sand_tint {
            Some((c, a)) => p.tinted(c, a),
            None => p,
        }
    };
    if h < -0.8 {
        return Some(sand(1.0));
    }
    let sl = slope(x, z, 2.0);
    if sl > 0.75 {
        let rock = OverlayPaint::layer("rock", 1.0);
        return Some(match biome {
            Biome::Arid => rock.tinted([0.42, 0.22, 0.13], 0.55),
            Biome::Arctic => rock.tinted([0.16, 0.17, 0.19], 0.4),
            _ => rock,
        });
    }
    if h < 2.2 && sl < 0.35 {
        if sand_tint.is_some() {
            return Some(sand(1.0));
        }
        let wet = smooth(1.6, 0.2, h);
        return Some(
            OverlayPaint::layer("sand", 1.0).tinted([0.42, 0.36, 0.27], 0.25 * wet as f32),
        );
    }
    if sl > 0.5 {
        return Some(OverlayPaint::layer("gravel", smooth(0.5, 0.75, sl) as f32));
    }
    let i = island_at(x, z)?;
    let isle = &islands()[i];
    let n = perlin2(x / 90.0, z / 90.0, 3) * 0.5 + 0.5;
    let wide = perlin2(x / 610.0, z / 610.0, 5) * 0.5 + 0.5;
    let wood = woods(x, z) as f32;
    let open = match biome {
        Biome::Temperate => {
            let f = isle.fertility;
            let lush = if f.grain || f.hops { 0.7 } else { 0.35 };
            // Meadows in three moods: deep green in the damp low ground, yellower where the
            // flowers stand, pale and dry on the hilltops.
            let green = [0.13 + 0.05 * n, 0.23 + 0.05 * lush as f32, 0.07 + 0.02 * n];
            let flowery = [0.30, 0.31, 0.09];
            let pale = [0.36, 0.33, 0.17];
            let patch = perlin2(x / 210.0, z / 210.0, 11) * 0.5 + 0.5;
            let a = smooth(0.52, 0.74, (patch * 0.7 + n * 0.3) as f64) as f32 * 0.8;
            let b = smooth(35.0, 120.0, h + 30.0 * (wide as f64 - 0.5)) as f32 * 0.75;
            let mut tint = green;
            for c in 0..3 {
                tint[c] += (flowery[c] - tint[c]) * a;
                tint[c] += (pale[c] - tint[c]) * b;
            }
            OverlayPaint::layer("grass", 1.0).tinted(tint, 0.42)
        }
        Biome::Tropical => {
            let tint = [0.07 + 0.05 * n, 0.21 + 0.06 * wide, 0.05 + 0.02 * n];
            OverlayPaint::layer("grass", 1.0).tinted(tint, 0.6)
        }
        Biome::Arid => {
            // Straw-coloured grass thinning to red earth on the heights.
            let bare = smooth(60.0, 200.0, h) as f32 * 0.7 + 0.3 * wide;
            let straw = [0.46 + 0.08 * n, 0.36 + 0.05 * n, 0.17];
            let earth = [0.44, 0.24, 0.13];
            let tint = [
                straw[0] + (earth[0] - straw[0]) * bare,
                straw[1] + (earth[1] - straw[1]) * bare,
                straw[2] + (earth[2] - straw[2]) * bare,
            ];
            if bare > 0.6 {
                OverlayPaint::layer("dirt", 1.0).tinted(tint, 0.75)
            } else {
                OverlayPaint::layer("grass", 1.0).tinted(tint, 0.8)
            }
        }
        Biome::Arctic => {
            // Snow on everything but the low, sheltered ground, which is tundra: rusty moss
            // and grey lichen in patches, with drifts left lying in the hollows.
            let fine = perlin2(x / 37.0, z / 37.0, 13) * 0.5 + 0.5;
            let lie = h + 22.0 * (wide as f64 - 0.5) + 9.0 * (n as f64 - 0.5);
            let snow = smooth(6.0, 26.0, lie) as f32;
            let drift = smooth(0.60, 0.70, (fine * 0.6 + n * 0.4) as f64) as f32;
            if snow.max(drift) > 0.5 {
                OverlayPaint::layer("snow", snow.max(drift))
            } else {
                let moss = [0.24 + 0.05 * n, 0.22 + 0.03 * n, 0.13];
                let rust = [0.34, 0.20, 0.10];
                let lichen = [0.33, 0.35, 0.29];
                let patch = perlin2(x / 150.0, z / 150.0, 17) * 0.5 + 0.5;
                let a = smooth(0.55, 0.75, patch as f64) as f32 * 0.7;
                let b =
                    smooth(0.55, 0.8, (1.0 - patch) as f64 * 0.6 + fine as f64 * 0.4) as f32 * 0.6;
                let mut tint = moss;
                for c in 0..3 {
                    tint[c] += (rust[c] - tint[c]) * a;
                    tint[c] += (lichen[c] - tint[c]) * b;
                }
                OverlayPaint::layer("grass", 1.0).tinted(tint, 0.75)
            }
        }
    };
    if wood > 0.45 {
        let floor = match biome {
            Biome::Temperate => [0.05, 0.10, 0.035],
            Biome::Tropical => [0.03, 0.11, 0.03],
            Biome::Arid => [0.20, 0.19, 0.08],
            Biome::Arctic => [0.07, 0.11, 0.08],
        };
        let k = smooth(0.45, 0.8, wood as f64) as f32;
        return Some(OverlayPaint::layer("leaves", 0.5 * k).tinted(floor, 0.3 + 0.5 * k));
    }
    Some(open)
}

/// The ground's paint under planet-local direction `d`.
pub fn ground_paint(d: DVec3) -> Option<engine_terrain::OverlayPaint> {
    let m = globe::map_of(d);
    if globe::reach_share(m.x, m.z) > 1.0 {
        return None;
    }
    paint(m.x, m.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settlers_land_on_a_shore_with_low_ground_up_to_the_town() {
        for isle in islands() {
            let Some(spot) = settle_spot(isle) else {
                continue;
            };
            let land = landing_coast(isle).unwrap();
            let steps = ((land - spot).length() / 10.0).ceil() as usize;
            let top = (0..steps)
                .map(|j| {
                    let p = spot + (land - spot) * (j as f64 / steps.max(1) as f64);
                    height(p.x, p.y)
                })
                .fold(0.0f64, f64::max);
            assert!(
                top <= 10.5,
                "{}: a rise of {top:.0} m behind the landing",
                isle.name
            );
        }
    }

    #[test]
    fn islands_rise_from_the_sea() {
        for isle in islands() {
            let c = isle.center;
            assert!(
                height(c.x, c.y) > 3.0,
                "{} middle {}",
                isle.name,
                height(c.x, c.y)
            );
            assert!(island_at(c.x, c.y).is_some(), "{}", isle.name);
            let out = c + DVec2::new(isle.radius * 2.2, 0.0);
            assert!(height(out.x, out.y) < -8.0, "{} offshore", isle.name);
        }
        assert!(height(900.0, 3400.0) < -20.0, "open sea between islands");
    }

    #[test]
    fn the_home_island_has_flat_land_for_a_town() {
        let spot = settle_spot(&islands()[0]).unwrap();
        let inland_dir = (islands()[0].center - spot).normalize();
        let mid = spot + inland_dir * 260.0;
        let mut flat = 0;
        for j in -40..40 {
            for i in -40..40 {
                let (x, z) = (mid.x + i as f64 * 8.0, mid.y + j as f64 * 8.0);
                if height(x, z) > 2.0 && slope(x, z, 4.0) < 0.08 {
                    flat += 1;
                }
            }
        }
        assert!(flat > 2500, "only {flat} flat samples");
    }

    /// A shaded relief of the whole map for eyeballing the coastlines:
    /// `ISLE_MAP=/tmp/map.ppm cargo test -p archipelago island::tests::relief -- --ignored`.
    #[test]
    #[ignore]
    fn relief() {
        let Ok(path) = std::env::var("ISLE_MAP") else {
            return;
        };
        let (n, half) = (900usize, 3600.0);
        let mut out = format!("P6 {n} {n} 255\n").into_bytes();
        for j in 0..n {
            for i in 0..n {
                let x = -half + 2.0 * half * i as f64 / n as f64;
                let z = -half + 2.0 * half * j as f64 / n as f64;
                let h = height(x, z);
                let shade = ((height(x - 4.0, z - 4.0) - h) * 0.25).clamp(-0.5, 0.5);
                let c = if h < 0.0 {
                    let d = (-h / 40.0).min(1.0);
                    [
                        40.0 + 90.0 * (1.0 - d),
                        90.0 + 100.0 * (1.0 - d),
                        150.0 + 70.0 * (1.0 - d),
                    ]
                } else if h < 2.0 {
                    [220.0, 205.0, 160.0]
                } else {
                    let k = (h / 120.0).min(1.0);
                    [90.0 + 120.0 * k, 140.0 + 40.0 * k, 70.0 + 110.0 * k]
                };
                for v in c {
                    out.push((v * (1.0 + shade)).clamp(0.0, 255.0) as u8);
                }
            }
        }
        std::fs::write(path, out).unwrap();
    }
}
