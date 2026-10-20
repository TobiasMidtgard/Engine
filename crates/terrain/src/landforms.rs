//! Named landforms laid out by latitude and longitude on top of the procedural shape: lowland
//! plains, broad rises, impact basins, shield volcanoes, rift canyons, labyrinths of troughs,
//! chaotic terrain, polar ice caps and dark albedo regions.
//!
//! [`PlanetShape::mars`](crate::shape::PlanetShape::mars) uses them to lay out the real planet:
//! the northern lowlands, the Tharsis rise with Olympus Mons and its neighbors, Valles
//! Marineris from Noctis Labyrinthus to the chaos at its mouth, Hellas, Argyre and Isidis, the
//! two polar caps with their spiral troughs, and Syrtis Major and the other dark regions.
//! Longitudes are degrees east, planet-local (see [`dir_from_lat_lon`]).

use engine_core::math::DVec3;
use engine_core::prelude::*;

use crate::noise;
use crate::shape::{dir_from_lat_lon, great_circle};

/// A place on the planet and its size.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Spot {
    pub lat_deg: f64,
    pub lon_deg: f64,
    /// Radius on the ground, meters.
    pub radius_m: f64,
}

impl Spot {
    pub const fn new(lat_deg: f64, lon_deg: f64, radius_m: f64) -> Self {
        Self {
            lat_deg,
            lon_deg,
            radius_m,
        }
    }

    pub fn direction(&self) -> DVec3 {
        dir_from_lat_lon(self.lat_deg, self.lon_deg)
    }

    /// Distance from the center in radii, if `dir` is within `reach` radii (cheap reject).
    fn x(&self, dir: DVec3, r: f64, reach: f64) -> Option<(f64, DVec3)> {
        let c = self.direction();
        // A chord is shorter than the arc, so this never rejects too much.
        if (dir - c).length() * r > self.radius_m * reach {
            return None;
        }
        Some((great_circle(dir, c, r) / self.radius_m, c))
    }
}

/// A shield volcano like Olympus Mons: a broad dome with gentle flanks streaked by lava flows,
/// a cliff around its foot (`scarp_m`), and nested collapse pits at the summit.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ShieldVolcano {
    pub at: Spot,
    /// Summit above the land around it, meters.
    pub height_m: f64,
    /// Radius of the summit caldera, meters.
    pub caldera_m: f64,
    /// Height of the cliff around the foot, meters (0 = none).
    pub scarp_m: f64,
}

/// A rift canyon (a chasma of Valles Marineris): a flat-floored trough with steep, spurred
/// walls along a path of `(lat, lon, half width m)` points.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Canyon {
    pub path: Vec<[f64; 3]>,
    /// Floor below the rim, meters.
    pub depth_m: f64,
}

/// An area broken into a maze of narrow troughs (Noctis Labyrinthus) or, with `chaos`, into
/// a sunken jumble of mesas and knobs (Aurorae Chaos).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Broken {
    pub at: Spot,
    pub depth_m: f64,
    /// Spacing of the troughs or blocks, meters.
    pub cell_m: f64,
    pub chaos: bool,
}

/// A polar cap of ice on a dome of layered deposits, cut by spiral troughs whose walls show
/// the dusty layers.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct IceCap {
    pub at: Spot,
    /// Height of the dome, meters.
    pub dome_m: f64,
    /// Number of spiral troughs around the cap (0 = none); negative turns them the other way.
    pub troughs: f64,
}

/// A dark region of the surface (Syrtis Major): dark sand and rock where the wind has swept
/// the bright dust away, with ragged, wind-streaked edges.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct DarkRegion {
    pub at: Spot,
    /// 0..1 how dark at its middle.
    pub darkness: f64,
}

/// Landforms on top of the procedural shape (all empty by default).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Landforms {
    /// Lowland hemispheres: smooth plains `depth_m` below the highlands (the radius is the
    /// edge's distance from the middle, measured along the ground).
    pub lowlands: Vec<(Spot, f64)>,
    /// Broad domes of uplifted crust, `(spot, height m)`.
    pub rises: Vec<(Spot, f64)>,
    /// Giant impact basins, `(spot, depth m)`: a flat floor, a ring of massifs and an apron.
    pub basins: Vec<(Spot, f64)>,
    pub volcanoes: Vec<ShieldVolcano>,
    pub canyons: Vec<Canyon>,
    pub broken: Vec<Broken>,
    pub ice_caps: Vec<IceCap>,
    pub dark_regions: Vec<DarkRegion>,
    /// 0..1 broad patches of darker and paler ground everywhere (bright dust over
    /// intermediate terrain).
    pub mottling: f64,
}

/// Ground resurfaced by lava recently enough to have gathered few craters since.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct YoungArea {
    /// Planet-local unit direction of the middle.
    pub center: DVec3,
    /// Angular radius, radians.
    pub angle: f64,
    /// 0..1 how few craters it keeps (1 = none).
    pub youth: f64,
}

/// What the landforms leave at a point besides height.
#[derive(Debug, Clone, Copy, Default)]
pub struct Marks {
    /// 0..1 dark ground (merged with maria).
    pub dark: f64,
    /// 0..1 ice of a polar cap.
    pub ice: f64,
}

fn smooth(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The youth one area gives `dir`: full out to three quarters of its radius, fading to none
/// at its edge.
pub fn youth_of(a: &YoungArea, dir: DVec3) -> f64 {
    let d = dir.dot(a.center);
    if d <= a.angle.cos() {
        return 0.0;
    }
    a.youth * (1.0 - smooth(0.75, 1.0, d.min(1.0).acos() / a.angle))
}

/// Two directions across the ground at `c`, for angles around it.
fn frame(c: DVec3) -> (DVec3, DVec3) {
    let e1 = DVec3::Y.cross(c).try_normalize().unwrap_or(DVec3::X);
    (e1, c.cross(e1))
}

impl Landforms {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The young ground: shield volcanoes and their aprons (almost no craters), the rises
    /// they stand on and the lowland plains (fewer).
    pub fn young_areas(&self, r: f64) -> impl Iterator<Item = YoungArea> + '_ {
        let area = move |at: &Spot, reach: f64, youth: f64| YoungArea {
            center: at.direction(),
            angle: at.radius_m * reach / r,
            youth,
        };
        self.volcanoes
            .iter()
            .map(move |v| area(&v.at, 1.15, 0.95))
            .chain(self.rises.iter().map(move |(at, _)| area(at, 1.0, 0.5)))
            .chain(self.lowlands.iter().map(move |(at, _)| area(at, 1.0, 0.6)))
    }

    /// How young the ground at `dir` is (0 old .. 1 fresh lava), which thins out its craters.
    /// `youth_at` in terrain.wgsl computes the same from [`young_areas`](Self::young_areas).
    pub fn youth(&self, dir: DVec3, r: f64) -> f64 {
        self.young_areas(r)
            .map(|a| youth_of(&a, dir))
            .fold(0.0, f64::max)
    }

    /// How far below and above the procedural shape the landforms can reach, meters
    /// (conservative: as if the deepest and highest of each kind were stacked).
    pub fn height_range(&self) -> (f64, f64) {
        let most = |it: &mut dyn Iterator<Item = f64>| it.fold(0.0, f64::max);
        let low = most(&mut self.lowlands.iter().map(|l| l.1))
            + most(&mut self.basins.iter().map(|b| b.1))
            + most(&mut self.canyons.iter().map(|c| c.depth_m))
            + most(&mut self.broken.iter().map(|b| b.depth_m))
            + most(&mut self.ice_caps.iter().map(|c| c.dome_m * 0.25));
        let high = most(&mut self.rises.iter().map(|r| r.1))
            + most(&mut self.basins.iter().map(|b| b.1 * 0.6))
            + most(&mut self.volcanoes.iter().map(|v| v.height_m + 150.0))
            + most(&mut self.ice_caps.iter().map(|c| c.dome_m));
        (-low, high)
    }

    /// Apply the landforms at unit direction `dir` on a planet of radius `r` to height `h`
    /// (meters), with detail no finer than `finest` meters.
    pub fn apply(&self, dir: DVec3, r: f64, finest: f64, seed: u64, h: &mut f64) -> Marks {
        let mut marks = Marks::default();
        if self.is_empty() {
            return marks;
        }
        let p = dir * r;
        let octaves = |scale: f64, max: f64| ((scale / finest).log2() + 1.0).clamp(0.0, max);
        let fbm = |scale: f64, salt: u64, max: f64| {
            noise::fbm_smooth(p / scale, seed ^ salt, octaves(scale, max), 0.5)
        };

        for (i, (at, depth)) in self.lowlands.iter().enumerate() {
            let c = at.direction();
            let salt = 0x10A0 + i as u64 * 8;
            // A ragged edge: bays, promontories and outliers of highland.
            let edge = at.radius_m * (1.0 + 0.12 * fbm(1_200_000.0, salt, 3.0))
                + 120_000.0 * fbm(250_000.0, salt + 1, 4.0);
            let d = great_circle(dir, c, r);
            let low = 1.0 - smooth(edge - 90_000.0, edge + 90_000.0, d);
            if low > 0.0 {
                // Lava and sediment smoothed the plains and buried most of their relief.
                *h = *h * (1.0 - 0.6 * low) - depth * low;
            }
        }

        for (at, height) in &self.rises {
            if let Some((x, _)) = at.x(dir, r, 1.0) {
                let t = 1.0 - x * x;
                *h += height * t * t;
            }
        }

        for (i, (at, depth)) in self.basins.iter().enumerate() {
            let Some((x, _)) = at.x(dir, r, 1.9) else {
                continue;
            };
            let salt = 0x20B0 + i as u64 * 8;
            let x = x + 0.07 * fbm(at.radius_m * 0.4, salt, 5.0);
            let floor = 1.0 - smooth(0.5, 0.95, x);
            let ring = (-((x - 1.05) / 0.13).powi(2)).exp();
            let massifs = if ring > 0.01 {
                let scale = at.radius_m * 0.05;
                noise::ridged_worn(p / scale, seed ^ (salt + 1), octaves(scale, 7.0), 0.6)
            } else {
                0.0
            };
            let apron = 1.0 - smooth(1.0, 1.9, x);
            // Dust and lava settled on the floor: smoother than the rim.
            *h = *h * (1.0 - 0.5 * floor) - depth * floor
                + depth * (0.3 * ring * (0.3 + 1.4 * massifs) + 0.06 * apron);
        }

        for (i, v) in self.volcanoes.iter().enumerate() {
            let Some((x, c)) = v.at.x(dir, r, 1.3) else {
                continue;
            };
            let salt = 0x30C0 + i as u64 * 8;
            let rad = v.at.radius_m;
            // Lobes of lava radiate from the summit: noise around the volcano, stretched along
            // the radius (on a circle in noise space, so there is no seam).
            let (e1, e2) = frame(c);
            let a = dir.dot(e2).atan2(dir.dot(e1));
            let around = rad / 9_000.0;
            let q = DVec3::new(a.cos() * around, a.sin() * around, x * rad / 40_000.0);
            let lobes = noise::fbm_smooth(q, seed ^ salt, octaves(9_000.0, 5.0), 0.55);
            let x = x + 0.04 * fbm(rad * 0.3, salt + 1, 4.0) + 0.01 * lobes;
            // Gentle flanks, steepest halfway down, and a summit plateau.
            let foot = if v.scarp_m > 0.0 { 0.93 } else { 1.0 };
            let xf = (x / foot).min(1.0);
            let flank = (1.0 - xf.powf(1.6)).max(0.0).powf(1.4);
            let cliff = 1.0 - smooth(foot - 0.01, foot + 0.035, x);
            let mut dh = (v.height_m - v.scarp_m) * flank + v.scarp_m * cliff;
            if x < 1.0 {
                dh += 120.0 * lobes * smooth(0.05, 0.5, x);
            }
            // Nested collapse pits at the summit, the youngest deepest.
            let rc = v.caldera_m;
            if rc > 0.0 && x * rad < rc * 2.5 {
                let off = e1 * (rc * 0.45 / r);
                for (k, (center, size, depth)) in [
                    (c, 1.0, 0.12),
                    ((c + off).normalize(), 0.6, 0.16),
                    (
                        (c - off * 0.7 + e2 * (rc * 0.3 / r)).normalize(),
                        0.45,
                        0.13,
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    let xc = great_circle(dir, center, r) / (rc * size)
                        + 0.04 * fbm(rc * 0.5, salt + 2 + k as u64, 3.0);
                    let pit = 1.0 - smooth(0.82, 1.0, xc);
                    dh = dh.min(v.height_m * (1.0 - depth * pit));
                }
            }
            *h += dh;
        }

        let mut floors = 0.0f64;
        for (i, canyon) in self.canyons.iter().enumerate() {
            let Some((dist, w)) = path_distance(&canyon.path, dir, r) else {
                continue;
            };
            let salt = 0x40D0 + i as u64 * 8;
            // Spurs and alcoves along the walls, scalloped by landslides.
            let wall = fbm(w * 0.5, salt, 6.0) * 0.18
                + noise::ridged_worn(
                    p / (w * 0.25),
                    seed ^ (salt + 1),
                    octaves(w * 0.25, 6.0),
                    0.3,
                ) * 0.12;
            // Bays and promontories a few widths long, so the rim is ragged from orbit too.
            let bays = fbm(w * 2.5, salt + 3, 3.0) * 0.22;
            let x = dist / w + wall + bays;
            // Walls about as steep everywhere (7 km down over ~13 km), however wide the floor.
            let wall_share = (canyon.depth_m * 1.8 / w).clamp(0.12, 0.3);
            let floor = 1.0 - smooth(1.0 - wall_share, 1.0, x);
            if floor > 0.0 {
                // Landslide aprons at the foot of the walls, and dunes and layered mounds on
                // the floor.
                let mounds =
                    smooth(0.15, 0.6, fbm(w * 0.8, salt + 2, 5.0)) * (1.0 - smooth(0.0, 0.6, x));
                let slides = smooth(0.45, 0.75, x) * 0.12;
                *h -= canyon.depth_m * floor * (1.0 - slides - 0.45 * mounds);
                floors = floors.max(floor);
                // Dark sand drifts across the floor between the pale mounds.
                marks.dark = marks.dark.max(0.6 * floor * (1.0 - mounds));
            }
        }

        for (i, b) in self.broken.iter().enumerate() {
            let Some((x, _)) = b.at.x(dir, r, 1.2) else {
                continue;
            };
            let salt = 0x50E0 + i as u64 * 8;
            let x = x + 0.12 * fbm(b.at.radius_m * 0.3, salt, 4.0);
            let mask = 1.0 - smooth(0.6, 1.0, x);
            if mask <= 0.0 {
                continue;
            }
            let n = fbm(b.cell_m, salt + 1, 6.0) + 0.15 * fbm(b.cell_m * 0.2, salt + 2, 5.0);
            if b.chaos {
                // Sunken, broken into mesas and knobs between a lattice of troughs.
                let blocks = smooth(-0.04, 0.08, n);
                *h -= b.depth_m * mask * (1.0 - 0.75 * blocks);
            } else {
                // The troughs follow where the noise crosses zero: a maze of intersecting
                // grabens.
                let trough = 1.0 - smooth(0.02, 0.07, n.abs());
                *h -= b.depth_m * mask * trough;
                floors = floors.max(mask * trough);
                marks.dark = marks.dark.max(0.4 * mask * trough);
            }
        }

        for (i, cap) in self.ice_caps.iter().enumerate() {
            let Some((x, c)) = cap.at.x(dir, r, 1.4) else {
                continue;
            };
            let salt = 0x60F0 + i as u64 * 8;
            let ragged = 0.08 * fbm(cap.at.radius_m * 0.25, salt, 6.0)
                + 0.04 * fbm(cap.at.radius_m * 0.05, salt + 1, 5.0);
            let x = x + ragged;
            let dome = (1.0 - (x / 1.15).powi(2)).max(0.0).powf(1.5);
            *h += cap.dome_m * dome;
            // The edge breaks up into patches of frost.
            let fringe = 0.15 * fbm(cap.at.radius_m * 0.03, salt + 5, 5.0);
            let mut ice = 1.0 - smooth(0.85, 1.05, x + fringe);
            if cap.troughs != 0.0 && ice > 0.0 {
                // Spiral troughs: lines of constant arms·angle + twist·distance, broken into
                // arcs, deepest in the middle of the cap.
                let (e1, e2) = frame(c);
                let a = dir.dot(e2).atan2(dir.dot(e1));
                let arms = cap.troughs.round();
                let twist = 7.0 * arms.abs();
                let phase = arms * a + twist * x + 0.8 * fbm(cap.at.radius_m * 0.4, salt + 2, 4.0);
                let line = 0.5 + 0.5 * phase.cos();
                // Long arcs, hundreds of kilometers, with an occasional gap.
                let arcs = smooth(-0.45, -0.1, fbm(cap.at.radius_m * 0.6, salt + 3, 3.0));
                let trough = smooth(0.55, 0.85, line) * arcs * smooth(0.1, 0.25, x) * ice;
                *h -= cap.dome_m * 0.25 * trough;
                // Their walls show dark, dusty layers of old ice.
                ice *= 1.0 - 0.7 * trough;
                marks.dark = marks.dark.max(0.7 * trough);
            }
            // Frost of uneven thickness, dust showing through in streaks.
            let frost = 0.6 + 0.4 * smooth(-0.3, 0.3, fbm(cap.at.radius_m * 0.08, salt + 4, 5.0));
            marks.ice = marks.ice.max(ice * frost * (1.0 - floors));
        }

        if self.mottling > 0.0 {
            let m = fbm(700_000.0, 0x7300, 4.0) + 0.4 * fbm(120_000.0, 0x7301, 5.0);
            marks.dark = marks.dark.max(0.45 * self.mottling * smooth(-0.1, 0.5, m));
        }
        for (i, region) in self.dark_regions.iter().enumerate() {
            let Some((x, _)) = region.at.x(dir, r, 1.9) else {
                continue;
            };
            let salt = 0x7100 + i as u64 * 8;
            let rad = region.at.radius_m;
            // Wind streaks: edges stretched east and west, the way the winds blow.
            let east = DVec3::Y.cross(dir).try_normalize().unwrap_or(DVec3::X);
            let q = p - east * (p.dot(east) * 0.7);
            let streaks = noise::fbm_smooth(
                q / (rad * 0.12),
                seed ^ salt,
                octaves(rad * 0.12, 6.0),
                0.55,
            );
            // A ragged shape with bays, islands of bright dust and outliers.
            let field = 0.9 - x + 0.8 * fbm(rad * 0.45, salt + 1, 5.0) + 0.35 * streaks;
            let tone = 0.75 + 0.25 * fbm(rad * 0.25, salt + 2, 4.0);
            let dark = region.darkness * tone * smooth(-0.05, 0.2, field);
            marks.dark = marks.dark.max(dark);
        }
        marks
    }
}

/// Distance from `dir` to a path of `(lat, lon, half width)` points along the ground, and the
/// half width there, if within two half widths.
fn path_distance(path: &[[f64; 3]], dir: DVec3, r: f64) -> Option<(f64, f64)> {
    let widest = path.iter().map(|q| q[2]).fold(0.0, f64::max);
    let mut best: Option<(f64, f64)> = None;
    for seg in path.windows(2) {
        let (a, b) = (
            dir_from_lat_lon(seg[0][0], seg[0][1]),
            dir_from_lat_lon(seg[1][0], seg[1][1]),
        );
        let len = great_circle(a, b, r);
        // Cheap reject: farther from both ends than the segment is long plus the margin.
        let reach = len + 2.5 * widest;
        if (dir - a).length() * r > reach && (dir - b).length() * r > reach {
            continue;
        }
        let n = a.cross(b).try_normalize().unwrap_or(DVec3::ZERO);
        let on = (dir - n * dir.dot(n)).try_normalize();
        let (d, t) = match on {
            Some(q) if a.cross(q).dot(n) >= 0.0 && q.cross(b).dot(n) >= 0.0 => (
                dir.dot(n).clamp(-1.0, 1.0).asin().abs() * r,
                great_circle(a, q, r) / len.max(1e-9),
            ),
            _ => {
                let (da, db) = (great_circle(dir, a, r), great_circle(dir, b, r));
                if da < db { (da, 0.0) } else { (db, 1.0) }
            }
        };
        let w = seg[0][2] + (seg[1][2] - seg[0][2]) * t;
        if d < 2.0 * w && best.is_none_or(|(bd, bw)| d / w < bd / bw) {
            best = Some((d, w));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f64 = 3_389_500.0;

    #[test]
    fn path_distance_measures_across_and_past_the_ends() {
        let path = [[0.0, 0.0, 50_000.0], [0.0, 10.0, 50_000.0]];
        // On the path's great circle, halfway: distance 0.
        let (d, w) = path_distance(&path, dir_from_lat_lon(0.0, 5.0), R).unwrap();
        assert!(d < 1.0 && (w - 50_000.0).abs() < 1.0, "{d} {w}");
        // One degree north of it: about 59 km.
        let (d, _) = path_distance(&path, dir_from_lat_lon(1.0, 5.0), R).unwrap();
        assert!((d - R * 1f64.to_radians()).abs() < 200.0, "{d}");
        // Past the end the distance is to the end point.
        let (d, _) = path_distance(&path, dir_from_lat_lon(0.0, 11.0), R).unwrap();
        assert!((d - R * 1f64.to_radians()).abs() < 200.0, "{d}");
        assert!(path_distance(&path, dir_from_lat_lon(30.0, 5.0), R).is_none());
    }

    #[test]
    fn volcano_summit_stands_high_with_a_caldera() {
        let forms = Landforms {
            volcanoes: vec![ShieldVolcano {
                at: Spot::new(10.0, 40.0, 300_000.0),
                height_m: 20_000.0,
                caldera_m: 40_000.0,
                scarp_m: 6_000.0,
            }],
            ..Default::default()
        };
        let height = |lat: f64, lon: f64| {
            let mut h = 0.0;
            forms.apply(dir_from_lat_lon(lat, lon), R, 100.0, 3, &mut h);
            h
        };
        let deg = 300_000.0 / R * 57.2958;
        let rim = height(10.0 + 50_000.0 / R * 57.2958, 40.0);
        let pit = height(10.0, 40.0);
        let flank = height(10.0 + 0.5 * deg, 40.0);
        let above_scarp = height(10.0 + 0.9 * deg, 40.0);
        let below_scarp = height(10.0 + 0.99 * deg, 40.0);
        assert!(rim > 17_000.0, "rim {rim}");
        assert!(pit < rim - 1_500.0, "pit {pit} rim {rim}");
        assert!(flank > 8_000.0 && flank < rim, "flank {flank}");
        assert!(
            above_scarp - below_scarp > 4_000.0,
            "{above_scarp} {below_scarp}"
        );
        assert!(height(10.0 + 1.4 * deg, 40.0).abs() < 1.0);
    }

    #[test]
    fn volcanoes_are_young_ground() {
        let forms = Landforms {
            volcanoes: vec![ShieldVolcano {
                at: Spot::new(0.0, 0.0, 100_000.0),
                height_m: 5_000.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let deg = 100_000.0 / R * 180.0 / std::f64::consts::PI;
        assert!((forms.youth(dir_from_lat_lon(0.0, 0.0), R) - 0.95).abs() < 1e-9);
        assert!(forms.youth(dir_from_lat_lon(0.0, 0.7 * deg), R) > 0.9);
        assert_eq!(forms.youth(dir_from_lat_lon(0.0, 1.2 * deg), R), 0.0);
        assert_eq!(Landforms::default().youth(DVec3::Y, R), 0.0);
    }

    #[test]
    fn canyons_cut_deep_and_ice_caps_are_icy() {
        let forms = Landforms {
            canyons: vec![Canyon {
                path: vec![[0.0, 0.0, 40_000.0], [0.0, 20.0, 60_000.0]],
                depth_m: 7_000.0,
            }],
            ice_caps: vec![IceCap {
                at: Spot::new(90.0, 0.0, 500_000.0),
                dome_m: 3_000.0,
                troughs: 0.0,
            }],
            ..Default::default()
        };
        // Across the floor: deep everywhere, down to the full depth between the mounds.
        let floor: Vec<f64> = (0..9)
            .map(|k| {
                let mut h = 0.0;
                forms.apply(
                    dir_from_lat_lon(-0.2 + 0.05 * k as f64, 10.0),
                    R,
                    100.0,
                    3,
                    &mut h,
                );
                h
            })
            .collect();
        assert!(floor.iter().all(|&h| h < -3_000.0), "{floor:?}");
        assert!(floor.iter().any(|&h| h < -6_000.0), "{floor:?}");
        let mut h = 0.0;
        forms.apply(dir_from_lat_lon(5.0, 10.0), R, 100.0, 3, &mut h);
        assert_eq!(h, 0.0);
        let mut h = 0.0;
        let m = forms.apply(DVec3::Y, R, 100.0, 3, &mut h);
        // Frost of uneven thickness: never less than 0.6 of full ice on the cap.
        assert!(m.ice > 0.59 && h > 2_500.0, "{} {h}", m.ice);
        let m = forms.apply(dir_from_lat_lon(70.0, 0.0), R, 100.0, 3, &mut 0.0);
        assert_eq!(m.ice, 0.0);
    }
}
