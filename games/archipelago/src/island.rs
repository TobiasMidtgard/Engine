//! The archipelago: where the islands are and the shape of their ground and sea bed.
//!
//! One pure height function describes every island and the sea floor between them. The
//! terrain module draws it (an overlay on a flat world whose own ground is all sea), the water
//! module floods it, and the build grid, carts, ships and the camera all read the same numbers,
//! so what a building stands on is what is drawn.

use engine::core::math::noise::{fbm2, perlin2};
use engine::prelude::*;

use crate::defs::Fertility;

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
}

/// The map. The home island is first and sits at the world origin.
pub fn islands() -> &'static [Island] {
    static ISLANDS: std::sync::OnceLock<Vec<Island>> = std::sync::OnceLock::new();
    ISLANDS.get_or_init(|| {
        vec![
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
            },
            Island {
                name: "Skerryvale",
                center: DVec2::new(-2150.0, -1500.0),
                radius: 420.0,
                hills_m: 62.0,
                cliff_dir: 4.0,
                cliff_share: 0.35,
                seed: 37,
                fertility: Fertility {
                    hops: true,
                    clay: true,
                    ..Default::default()
                },
                woods: 0.5,
                settle_toward: Some(0),
                settle_m: 240.0,
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
                    ..Default::default()
                },
                woods: 0.55,
                settle_toward: Some(0),
                settle_m: 300.0,
            },
            Island {
                name: "Little Tarn",
                center: DVec2::new(1750.0, 1650.0),
                radius: 260.0,
                hills_m: 58.0,
                cliff_dir: 5.2,
                cliff_share: 0.6,
                seed: 53,
                fertility: Fertility::default(),
                woods: 0.7,
                settle_toward: None,
                settle_m: 0.0,
            },
        ]
    })
}

/// How far the hand-made ground reaches from the origin before it blends into open sea.
pub const MAP_RADIUS_M: f64 = 5600.0;
/// The sea floor far from every island, meters.
pub const OPEN_SEA_M: f64 = -42.0;

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
    let wild = 1.0 - calm(isle, x, z, 1.0);
    // Coastline: lobes (peninsulas and bights), then bays and headlands from warped noise.
    let lobe = lobes(isle, d.y.atan2(d.x)) * wild;
    let warp = fbm2(x / 520.0, z / 520.0, s, 3, 2.0, 0.5) as f64 * 0.34
        + fbm2(x / 140.0, z / 140.0, s + 1, 3, 2.1, 0.5) as f64 * 0.07;
    // Coves: narrow inlets where a noise field crosses zero, only in the outer band and only
    // on some stretches of coast.
    let n = fbm2(x / 340.0, z / 340.0, s + 11, 3, 2.0, 0.5) as f64;
    let channel = (1.0 - n.abs() * 4.0).max(0.0).powi(2);
    let stretch = smooth(-0.15, 0.15, perlin2(x / 800.0, z / 800.0, s + 12) as f64);
    let band = smooth(0.45, 0.8, r) * smooth(1.25, 1.0, r);
    let cove = 0.4 * channel * stretch * band * wild;
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
    let wobble = perlin2(x / 260.0, z / 260.0, isle.seed + 5) as f64 * 0.35;
    let run = smooth(half, half * 0.55, off.abs() + wobble * half);
    let headland = smooth(0.06, 0.16, lobes(isle, a));
    run.max(headland * 0.85) * (1.0 - calm(isle, x, z, 1.25))
}

/// Height of one island's ground (or its sea bed), meters.
fn island_height(isle: &Island, x: f64, z: f64) -> f64 {
    let t = inland(isle, x, z);
    let s = isle.seed;
    let c = cliff(isle, x, z);
    if t < 0.0 {
        // Sea bed: a shelf, then down to the open sea.
        let shelf = -1.5 - 9.0 * smooth(0.0, 0.08, -t);
        let deep = open_sea(x, z);
        // Under cliffs the bed drops away fast; elsewhere a sandy shelf runs out.
        let k = smooth(0.0, 0.55 - 0.45 * c, -t);
        return shelf + (deep - shelf) * k;
    }
    // Beach then a low bank up to the plateau; cliffs instead rise sheer from the water.
    let beach_w = 0.07 * (1.0 - c) + 0.006 * c;
    let beach = 1.6 * smooth(0.0, beach_w * 0.6, t);
    // Cliff tops are high downs that settle back into the rolling land further in.
    let bank_top = 5.0 + 24.0 * c * smooth(0.4, 0.08, t);
    let bank = bank_top * smooth(beach_w * 0.5, beach_w + 0.05 * (1.0 - c) + 0.01, t);
    // Rolling land: what towns are built on; calmer near the settlement.
    let town = calm(isle, x, z, 1.35);
    let roll = fbm2(x / 380.0, z / 380.0, s + 2, 3, 2.0, 0.5) as f64 * (4.0 - 2.4 * town);
    let land = smooth(0.02, 0.12, t);
    // Hills where the hill mask says so, ridged, and a high middle on every island.
    let mask = smooth(
        0.0,
        0.4,
        fbm2(x / 700.0, z / 700.0, s + 3, 2, 2.0, 0.5) as f64 + 0.2,
    );
    let ridge = {
        let n = fbm2(x / 260.0, z / 260.0, s + 4, 4, 2.05, 0.5) as f64;
        (1.0 - n.abs() * 1.6).max(0.0).powi(2)
    };
    // The high middle follows plain distance (warped), not the lobed coast, so it has no
    // creases running out to the headlands.
    let rr = (DVec2::new(x, z) - isle.center).length() / isle.radius
        + fbm2(x / 450.0, z / 450.0, s + 6, 3, 2.0, 0.5) as f64 * 0.3;
    let crown = smooth(0.8, 0.1, rr) * (0.6 + 0.4 * ridge);
    let hills = isle.hills_m
        * (mask * smooth(0.15, 0.6, t) * (0.3 + 0.7 * ridge) * 0.75 + 0.45 * crown)
        * (1.0 - town);
    bank.max(beach) + land * (roll + hills)
}

/// The sea floor away from the islands.
fn open_sea(x: f64, z: f64) -> f64 {
    OPEN_SEA_M + 3.0 * perlin2(x / 1500.0, z / 1500.0, 99) as f64
}

/// World height of the ground or sea bed at world `(x, z)`, meters (sea level 0).
pub fn height(x: f64, z: f64) -> f64 {
    let mut h = open_sea(x, z);
    for isle in islands() {
        let d2 = (DVec2::new(x, z) - isle.center).length_squared();
        if d2 > (isle.radius * 3.2).powi(2) {
            continue;
        }
        h = h.max(island_height(isle, x, z));
    }
    h
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
    if h < 2.5 || t < 0.06 {
        return 0.0;
    }
    let n = fbm2(x / 330.0, z / 330.0, isle.seed + 7, 3, 2.0, 0.5) as f64
        + fbm2(x / 70.0, z / 70.0, isle.seed + 8, 2, 2.0, 0.5) as f64 * 0.25;
    // Hills are wooded more often than flats; the home island's middle is kept open for the town.
    let hilly = smooth(8.0, 25.0, h) * 0.35;
    let open_town = if i == 0 {
        smooth(300.0, 170.0, (DVec2::new(x, z) - home_town_spot()).length())
    } else {
        0.0
    };
    smooth(0.55 - isle.woods, 0.75 - isle.woods, n + hilly) * (1.0 - open_town)
}

/// Ground layer and tint for the terrain's paint: sand on beaches, rock on cliffs, meadow
/// tones on the plateau (fertile islands greener).
pub fn paint(x: f64, z: f64) -> Option<engine_terrain::OverlayPaint> {
    use engine_terrain::OverlayPaint;
    let h = height(x, z);
    if h < -0.8 {
        return Some(OverlayPaint::layer("sand", 1.0));
    }
    let sl = slope(x, z, 2.0);
    if sl > 0.75 {
        return Some(OverlayPaint::layer("rock", 1.0));
    }
    if h < 2.2 && sl < 0.35 {
        let wet = smooth(1.6, 0.2, h);
        return Some(
            OverlayPaint::layer("sand", 1.0).tinted([0.42, 0.36, 0.27], 0.25 * wet as f32),
        );
    }
    if sl > 0.5 {
        return Some(OverlayPaint::layer("gravel", smooth(0.5, 0.75, sl) as f32));
    }
    let i = island_at(x, z)?;
    let f = islands()[i].fertility;
    let lush = if f.grain || f.hops { 0.7 } else { 0.35 };
    let n = perlin2(x / 90.0, z / 90.0, 3) * 0.5 + 0.5;
    let tint = [0.16 + 0.06 * n, 0.22 + 0.05 * lush as f32, 0.08 + 0.02 * n];
    Some(OverlayPaint::layer("grass", 1.0).tinted(tint, 0.35))
}

#[cfg(test)]
mod tests {
    use super::*;

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
