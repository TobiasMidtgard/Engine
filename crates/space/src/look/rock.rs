//! Rocky bodies: the Moon, Mercury, Mars and a generic cratered rock.
//!
//! Craters are scattered on a 3D grid over the sphere at several scales (each cell may hold
//! one crater, so the cost per texel is fixed), with bowl-and-rim relief, bright ejecta on the
//! fresh ones and long bright rays from a few young named craters. Maria and albedo regions
//! are lists of named shapes with noisy edges at their real coordinates.

use std::f32::consts::PI;

use engine_core::math::{Rng, Vec3};

use super::bake::{Paint, Painter, Texel, par_rows};
use super::color::{Rgb, mix, mul, scale, srgb};
use super::noise::{Grid, Noise, angle_between, dir, hash3, local_offset, smooth, unit, wrap_pi};

const DEG: f32 = PI / 180.0;

/// One scale of the crater population.
#[derive(Debug, Clone, Copy)]
struct CraterLevel {
    /// Typical spacing on the unit sphere (≈ radians): the level holds `density` craters per
    /// `cell`² of surface.
    cell: f32,
    density: f32,
    /// Crater radius range as a fraction of the cell.
    radius: (f32, f32),
    /// Fraction of craters that are fresh (sharp, bright ejecta).
    fresh: f32,
    seed: u32,
}

/// What craters add at a point.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct CraterSample {
    /// Height in body radii.
    pub height: f32,
    /// Albedo factor offset (0 = unchanged, + brighter).
    pub bright: f32,
}

/// How far a crater reaches (rim and ejecta), in crater radii.
const CRATER_REACH: f32 = 2.6;

/// One crater on the unit sphere.
#[derive(Debug, Clone, Copy)]
struct Crater {
    center: Vec3,
    /// Radius, radians.
    rad: f32,
    depth: f32,
    rim: f32,
    fresh: bool,
}

impl Crater {
    /// Height (body radii) and brightness offset at `r` crater radii from the center.
    #[inline]
    fn profile(&self, r: f32) -> (f32, f32) {
        let big = self.rad > 0.02;
        let height = if r < 1.0 {
            // Bowl with a flat-ish floor for the big ones.
            let floor = if big { 0.35 } else { 0.0 };
            let b = ((r - floor).max(0.0) / (1.0 - floor)).powi(2);
            -self.depth + (self.depth + self.rim) * b
        } else {
            self.rim * (-(r - 1.0) * 3.2).exp()
        };
        // Central peak in large craters.
        let peak = if big && r < 0.25 {
            0.25 * self.depth * (1.0 - r / 0.25)
        } else {
            0.0
        };
        let bright = if self.fresh {
            if r < 1.0 {
                0.35 + 0.45 * smooth(0.5, 1.0, r)
            } else {
                0.8 * (-(r - 1.0) * 1.9).exp() * smooth(CRATER_REACH, CRATER_REACH * 0.7, r)
            }
        } else if r < 1.0 {
            0.05 * (r - 0.7)
        } else {
            0.0
        };
        ((height + peak) * self.rad, bright)
    }
}

/// A population of craters at several scales.
pub(crate) struct Craters {
    levels: Vec<CraterLevel>,
    /// Depth of a simple crater per unit radius.
    depth: f32,
}

/// Craters splatted into a map the size of the bake: look them up per texel.
pub(crate) struct CraterMap {
    width: usize,
    height: usize,
    /// Height, brightness.
    data: Vec<[f32; 2]>,
}

impl CraterMap {
    #[inline]
    pub(crate) fn at(&self, t: &Texel) -> CraterSample {
        let x = (t.x as usize).min(self.width - 1);
        let y = (t.y as usize).min(self.height - 1);
        let [height, bright] = self.data[y * self.width + x];
        CraterSample { height, bright }
    }
}

impl Craters {
    /// Craters from `largest` (radians, spacing of the biggest level) down to what a map
    /// `width` texels wide resolves.
    pub(crate) fn new(largest: f32, density: f32, fresh: f32, width: u32, seed: u32) -> Self {
        let texel = std::f32::consts::TAU / width as f32;
        let mut levels = Vec::new();
        let mut cell = largest;
        let mut i = 0;
        // The smallest craters are about a texel across.
        while cell * 0.15 > texel * 0.5 && i < 9 {
            levels.push(CraterLevel {
                cell,
                density: (density * (0.75 + 0.07 * i as f32)).min(0.95),
                radius: (0.07, 0.23),
                fresh: fresh * (1.0 + 0.3 * i as f32),
                seed: seed.wrapping_add(i * 7919),
            });
            cell *= 0.45;
            i += 1;
        }
        Self {
            levels,
            depth: 0.35,
        }
    }

    /// Scatter the craters and splat their relief and brightness into a map `width` texels
    /// wide (rows from the north pole). `keep(center)` (0..1) thins them out, as on young
    /// plains. Costs about the area the craters cover, not texels × craters.
    pub(crate) fn bake(&self, width: u32, keep: impl Fn(Vec3) -> f32) -> CraterMap {
        use std::f32::consts::{FRAC_PI_2, TAU};
        let w = width.max(4) as usize;
        let h = w / 2;
        let texel = TAU / w as f32;
        let mut list = Vec::new();
        for l in &self.levels {
            let count = (l.density * 4.0 * PI / (l.cell * l.cell)) as usize;
            let mut rng = Rng::new(0x00c4_a7e5 ^ l.seed as u64);
            for _ in 0..count {
                let z = rng.range(-1.0, 1.0);
                let phi = rng.range(-PI, PI);
                let s = (1.0 - z * z).max(0.0).sqrt();
                let center = Vec3::new(s * phi.sin(), z, s * phi.cos());
                let size = rng.f32();
                let rad = l.cell * (l.radius.0 + (l.radius.1 - l.radius.0) * size * size);
                let fresh = rng.f32() < l.fresh;
                let age = if fresh { 1.0 } else { 0.35 + 0.5 * rng.f32() };
                let roll = rng.f32();
                if rad < texel * 0.5 || roll > keep(center) {
                    continue;
                }
                list.push(Crater {
                    center,
                    rad,
                    depth: self.depth * age,
                    rim: 0.09 * age,
                    fresh,
                });
            }
        }
        // Bucket the craters by the bands of rows they touch.
        const BAND: usize = 8;
        let mut buckets = vec![Vec::new(); h.div_ceil(BAND)];
        let row_of = |lat: f32| (FRAC_PI_2 - lat) / PI * h as f32;
        for (i, c) in list.iter().enumerate() {
            let lat = c.center.y.clamp(-1.0, 1.0).asin();
            let reach = c.rad * CRATER_REACH;
            let y0 = (row_of(lat + reach) - 1.0).max(0.0) as usize;
            let y1 = ((row_of(lat - reach) + 1.0).max(0.0) as usize).min(h - 1);
            for b in &mut buckets[y0 / BAND..=y1 / BAND] {
                b.push(i as u32);
            }
        }
        let cols: Vec<(f32, f32)> = (0..w)
            .map(|x| ((x as f32 + 0.5) / w as f32 * TAU - PI).sin_cos())
            .collect();
        let mut data = vec![[0.0f32; 2]; w * h];
        par_rows(&mut data, w, |y, row| {
            let lat = FRAC_PI_2 - (y as f32 + 0.5) / h as f32 * PI;
            let (sl, cl) = lat.sin_cos();
            for &i in &buckets[y / BAND] {
                let c = &list[i as usize];
                let reach = c.rad * CRATER_REACH;
                // The crater's longitude span on this row.
                let (cs, cc) = (c.center.y, (1.0 - c.center.y * c.center.y).max(0.0).sqrt());
                let denom = cl * cc;
                let cos_span = (reach.cos() - sl * cs) / denom.max(1e-9);
                if cos_span > 1.0 {
                    continue;
                }
                let lon = c.center.x.atan2(c.center.z);
                let span = if cos_span <= -1.0 {
                    PI
                } else {
                    cos_span.acos()
                };
                let x0 = ((lon - span + PI) / TAU * w as f32).floor() as i64 - 1;
                let x1 = ((lon + span + PI) / TAU * w as f32).ceil() as i64 + 1;
                let (x0, x1) = if x1 - x0 >= w as i64 {
                    (0, w as i64 - 1)
                } else {
                    (x0, x1)
                };
                for xi in x0..=x1 {
                    let x = xi.rem_euclid(w as i64) as usize;
                    let (so, co) = cols[x];
                    let d = Vec3::new(cl * so, sl, cl * co);
                    let r = (d - c.center).length() / c.rad;
                    if r < CRATER_REACH {
                        let (dh, db) = c.profile(r);
                        row[x][0] += dh;
                        row[x][1] += db;
                    }
                }
            }
        });
        CraterMap {
            width: w,
            height: h,
            data,
        }
    }
}

/// A young crater whose bright rays reach far across the surface.
#[derive(Debug, Clone, Copy)]
struct RayCrater {
    center: Vec3,
    /// Crater radius, radians.
    radius: f32,
    /// How far the rays reach, in crater radii.
    reach: f32,
    /// Cosine of the reach angle (a quick test before the real work).
    cos_reach: f32,
    strength: f32,
    seed: u32,
}

impl RayCrater {
    fn new(
        lat_deg: f32,
        lon_deg: f32,
        radius_km: f32,
        body_km: f32,
        reach: f32,
        strength: f32,
        seed: u32,
    ) -> Self {
        Self {
            center: dir(lat_deg * DEG, lon_deg * DEG),
            radius: radius_km / body_km,
            reach,
            cos_reach: (radius_km / body_km * reach).min(PI).cos(),
            strength,
            seed,
        }
    }

    /// Brightness added at `d` (0 = none) and the crater relief (height in body radii).
    #[inline]
    fn sample(&self, d: Vec3, n: &Noise) -> (f32, f32) {
        if d.dot(self.center) < self.cos_reach {
            return (0.0, 0.0);
        }
        let ang = angle_between(d, self.center);
        let r = ang / self.radius;
        if r > self.reach {
            return (0.0, 0.0);
        }
        let (e, no) = local_offset(d, self.center);
        let bearing = e.atan2(no);
        // Rays: sharp peaks of a periodic 1D noise in the bearing.
        let mut rays = 0.0;
        for (k, freq) in [(0u32, 9.0f32), (1, 23.0), (2, 47.0)] {
            let x = (bearing / std::f32::consts::TAU + 0.5) * freq;
            let i = super::noise::floor_i(x) as f32;
            let f = x - i;
            let a = unit(hash3(i as i32 % freq as i32, k as i32, 0, self.seed), 0);
            let b = unit(
                hash3((i as i32 + 1) % freq as i32, k as i32, 0, self.seed),
                0,
            );
            let v = a + (b - a) * (f * f * (3.0 - 2.0 * f));
            rays += smooth(0.55, 0.95, v) * [0.6, 0.35, 0.25][k as usize];
        }
        // Rays break up along their length.
        let blotch = 0.55 + 0.6 * n.at(d * 60.0 + Vec3::splat(self.seed as f32 * 0.01));
        let fade = (1.0 - r / self.reach).max(0.0).powf(1.4);
        let halo = (-(r - 1.0).max(0.0) * 0.9).exp();
        let bright = self.strength
            * (rays * fade * blotch.max(0.0) * smooth(1.0, 2.0, r)
                + 0.9 * halo * smooth(0.6, 1.1, r));
        let relief = if r < 1.0 {
            (-0.3 + 0.39 * r * r) * self.radius
        } else {
            0.09 * self.radius * (-(r - 1.0) * 3.0).exp()
        };
        (bright, relief)
    }
}

/// A dark (or bright) region with a noisy outline: maria, Martian albedo features, basins.
#[derive(Debug, Clone, Copy)]
struct Region {
    center: Vec3,
    /// Angular radius north-south (radians) and east-west elongation.
    radius: f32,
    stretch: f32,
    /// Outline roughness (fraction of the radius).
    rough: f32,
    strength: f32,
}

impl Region {
    fn new(
        lat_deg: f32,
        lon_deg: f32,
        radius_deg: f32,
        stretch: f32,
        rough: f32,
        strength: f32,
    ) -> Self {
        Self {
            center: dir(lat_deg * DEG, lon_deg * DEG),
            radius: radius_deg * DEG,
            stretch,
            rough,
            strength,
        }
    }

    /// Coverage 0..1 at `d`, with a soft edge. Painters evaluate it on a coarse grid and
    /// sharpen the edge per texel with [`ragged`].
    #[inline]
    fn cover(&self, d: Vec3, n: &Noise, soft: f32) -> f32 {
        if d.dot(self.center) < (self.radius * self.stretch.max(1.0) * 1.8).min(PI).cos() {
            return 0.0;
        }
        let (e, no) = local_offset(d, self.center);
        let r = ((e / self.stretch).powi(2) + no * no).sqrt() / self.radius;
        let edge = 1.0 + self.rough * n.fbm(d * (3.0 / self.radius.max(0.02)).min(40.0), 4, 0.6);
        smooth(edge + soft, edge - soft, r) * self.strength
    }
}

/// A coverage interpolated from a coarse grid, its edge made ragged and sharper by fine noise
/// (`fine` about −1..1) so outlines keep detail below the grid's cells.
#[inline]
pub(crate) fn ragged(cover: f32, fine: f32, amount: f32) -> f32 {
    let c = cover.clamp(0.0, 1.0);
    let edge = c * (1.0 - c) * 4.0;
    smooth(0.0, 1.0, c + amount * edge * fine)
}

/// Grid size for a painter's coarse fields at map `width`.
pub(crate) fn grid_width(width: u32) -> usize {
    (width as usize / 4).clamp(64, 512)
}

fn union(a: f32, b: f32) -> f32 {
    1.0 - (1.0 - a) * (1.0 - b.clamp(0.0, 1.0))
}

// --------------------------------------------------------------------------------------- Moon

/// The Moon: bright cratered highlands, dark maria in their real places on the near side
/// (longitude 0 faces Earth), few maria on the far side, and the bright rays of Tycho,
/// Copernicus, Kepler and others.
pub(crate) struct Moon {
    n: Noise,
    n2: Noise,
    /// Mare cover, mare color (rgb), South Pole–Aitken, coarse mottling.
    coarse: Grid<6>,
    craters: CraterMap,
    rays: Vec<RayCrater>,
}

const MOON_KM: f32 = 1737.4;

impl Moon {
    pub(crate) fn new(seed: u64, width: u32) -> Self {
        let blue = srgb!(0x4f5155);
        let brown = srgb!(0x57524b);
        let gray = srgb!(0x535252);
        let maria = vec![
            // Oceanus Procellarum: a sprawl of overlapping lobes.
            (Region::new(18.0, -57.0, 20.0, 0.8, 0.35, 1.0), gray),
            (Region::new(-2.0, -45.0, 12.0, 1.0, 0.35, 1.0), gray),
            (Region::new(35.0, -45.0, 13.0, 1.2, 0.35, 1.0), brown),
            (Region::new(5.0, -65.0, 10.0, 0.8, 0.4, 0.9), gray),
            (Region::new(33.0, -16.0, 17.0, 1.1, 0.12, 1.0), brown),
            (Region::new(28.0, 17.5, 10.5, 1.0, 0.1, 1.0), brown),
            (Region::new(8.5, 31.0, 12.0, 1.1, 0.3, 1.0), blue),
            (Region::new(17.0, 59.0, 7.0, 1.3, 0.08, 1.0), brown),
            (Region::new(-7.8, 51.3, 10.0, 0.8, 0.35, 0.95), gray),
            (Region::new(-15.2, 35.5, 5.2, 1.0, 0.15, 0.95), gray),
            (Region::new(-21.0, -17.0, 10.5, 1.1, 0.35, 0.9), gray),
            (Region::new(-24.5, -39.5, 6.0, 1.0, 0.12, 1.0), brown),
            (Region::new(56.0, 1.0, 4.5, 8.0, 0.3, 0.85), gray),
            (Region::new(13.3, 3.6, 3.8, 1.0, 0.3, 0.9), brown),
            (Region::new(7.5, -30.9, 7.0, 1.0, 0.35, 0.85), gray),
            (Region::new(-10.0, -23.0, 4.5, 1.0, 0.3, 0.85), gray),
            (Region::new(2.0, 2.0, 3.0, 1.2, 0.35, 0.7), gray),
            // Far side: Moscoviense, Ingenii, Tsiolkovskiy's floor, Orientale's rings.
            (Region::new(27.3, 147.9, 4.2, 1.0, 0.25, 0.85), gray),
            (Region::new(-33.7, 163.5, 3.0, 1.1, 0.4, 0.6), gray),
            (Region::new(-20.4, 129.1, 1.8, 1.0, 0.2, 0.9), blue),
            (Region::new(-19.4, -92.8, 3.5, 1.0, 0.3, 0.7), gray),
        ];
        let rays = vec![
            RayCrater::new(-43.3, -11.2, 43.0, MOON_KM, 38.0, 1.0, 11),
            RayCrater::new(9.6, -20.1, 46.0, MOON_KM, 22.0, 0.8, 12),
            RayCrater::new(8.1, -38.0, 16.0, MOON_KM, 30.0, 0.7, 13),
            RayCrater::new(23.7, -47.4, 20.0, MOON_KM, 18.0, 0.9, 14),
            RayCrater::new(16.1, 46.8, 14.0, MOON_KM, 25.0, 0.6, 15),
            RayCrater::new(36.0, 102.9, 11.0, MOON_KM, 40.0, 0.9, 16),
            RayCrater::new(22.4, -163.1, 36.0, MOON_KM, 20.0, 0.7, 17),
            RayCrater::new(-8.9, 61.1, 66.0, MOON_KM, 8.0, 0.4, 18),
            RayCrater::new(-11.3, 121.8, 15.0, MOON_KM, 18.0, 0.6, 19),
            RayCrater::new(52.0, -110.0, 12.0, MOON_KM, 20.0, 0.5, 20),
        ];
        // South Pole-Aitken: huge, slightly darker and low.
        let basin = Region::new(-53.0, -169.0, 34.0, 1.0, 0.15, 0.5);
        let n = Noise::new(seed);
        let n2 = Noise::new(seed ^ 0x3c);
        let gw = grid_width(width);
        let coarse = Grid::bake(gw, gw / 2, |d| {
            let mut mare = 0.0f32;
            let mut color = srgb!(0x535252);
            for (r, c) in &maria {
                let k = r.cover(d, &n, 0.06);
                if k > 0.0 {
                    color = mix(color, *c, k / (mare + k).max(1e-3));
                    mare = union(mare, k);
                }
            }
            let spa = basin.cover(d, &n, 0.3);
            [
                mare,
                color[0],
                color[1],
                color[2],
                spa,
                n2.fbm(d * 7.0, 3, 0.6),
            ]
        });
        let craters = Craters::new(18.0 * DEG, 0.55, 0.06, width, seed as u32)
            .bake(width, |c| 1.0 - 0.75 * coarse.at_dir(c)[0]);
        Self {
            n,
            n2,
            coarse,
            craters,
            rays,
        }
    }
}

impl Painter for Moon {
    fn paint(&self, t: &Texel) -> Paint {
        let d = t.d;
        let [mare, r, g, b, spa, mottle] = self.coarse.at(t.lat, t.lon);
        let mare = ragged(mare, self.n.fbm(d * 48.0, 2, 0.5), 0.7);
        let mare_color = [r, g, b];
        let cr = self.craters.at(t);
        let mut height = cr.height - 0.0012 * mare - 0.004 * spa;
        let mottle = mottle + 0.35 * self.n2.fbm(d * 56.0, 2, 0.6);
        let highland = scale(srgb!(0x7c7870), 1.0 + 0.1 * mottle - 0.06 * spa);
        let lowland = scale(mare_color, 1.0 + 0.07 * mottle);
        let mut c = mix(highland, lowland, mare);
        c = scale(c, 1.0 + 0.45 * cr.bright * (1.0 - 0.4 * mare));
        let mut ray = 0.0;
        for r in &self.rays {
            let (b, h) = r.sample(d, &self.n2);
            ray += b;
            height += h;
        }
        c = scale(c, 1.0 + 0.55 * ray.min(1.5));
        let grain = 1.0 + 0.04 * self.n.at(d * 150.0);
        Paint {
            color: scale(c, grain),
            height,
            ..Default::default()
        }
    }

    fn relief(&self) -> bool {
        true
    }
}

// ------------------------------------------------------------------------------------ Mercury

/// Mercury: darker, browner and more evenly cratered than the Moon, with the Caloris basin,
/// smooth plains and bright ray craters.
pub(crate) struct Mercury {
    n: Noise,
    n2: Noise,
    /// Smooth plains, Caloris, low-reflectance material, coarse mottling.
    coarse: Grid<4>,
    craters: CraterMap,
    rays: Vec<RayCrater>,
}

const MERCURY_KM: f32 = 2439.7;

impl Mercury {
    pub(crate) fn new(seed: u64, width: u32) -> Self {
        let n = Noise::new(seed);
        let n2 = Noise::new(seed ^ 0x4d);
        let plains = [
            Region::new(65.0, 30.0, 18.0, 2.0, 0.4, 0.8),
            Region::new(20.0, -40.0, 9.0, 1.2, 0.4, 0.6),
            Region::new(-30.0, 120.0, 10.0, 1.0, 0.4, 0.5),
        ];
        let caloris = Region::new(30.5, 170.2, 13.0, 1.0, 0.08, 1.0);
        let gw = grid_width(width);
        let coarse = Grid::bake(gw, gw / 2, |d| {
            let mut plain = 0.0f32;
            for r in &plains {
                plain = union(plain, r.cover(d, &n, 0.15));
            }
            let cal = caloris.cover(d, &n, 0.04);
            // Dark, bluish low-reflectance material in patches.
            let lrm = smooth(0.25, 0.6, n2.fbm(d * 3.0 + Vec3::splat(4.0), 4, 0.55));
            [plain, cal, lrm, n2.fbm(d * 6.0, 3, 0.6)]
        });
        let craters = Craters::new(14.0 * DEG, 0.62, 0.07, width, seed as u32).bake(width, |c| {
            let [plain, cal, ..] = coarse.at_dir(c);
            1.0 - 0.6 * plain.max(cal)
        });
        Self {
            n,
            n2,
            coarse,
            craters,
            rays: vec![
                RayCrater::new(57.8, 16.8, 47.0, MERCURY_KM, 22.0, 0.9, 31),
                RayCrater::new(-11.4, -31.5, 31.0, MERCURY_KM, 18.0, 0.8, 32),
                RayCrater::new(-33.9, 12.6, 40.0, MERCURY_KM, 25.0, 0.9, 33),
                RayCrater::new(10.0, -100.0, 25.0, MERCURY_KM, 15.0, 0.6, 34),
                RayCrater::new(-60.0, 150.0, 30.0, MERCURY_KM, 16.0, 0.6, 35),
            ],
        }
    }
}

impl Painter for Mercury {
    fn paint(&self, t: &Texel) -> Paint {
        let d = t.d;
        let [plain, cal, lrm, mottle] = self.coarse.at(t.lat, t.lon);
        let fine = self.n.fbm(d * 48.0, 2, 0.5);
        let (plain, cal) = (ragged(plain, fine, 0.6), ragged(cal, fine, 0.4));
        let cr = self.craters.at(t);
        let mottle = mottle + 0.35 * self.n2.fbm(d * 48.0, 2, 0.6);
        let mut c = scale(srgb!(0x6c6863), 1.0 + 0.1 * mottle);
        c = mix(c, srgb!(0x56585a), lrm * 0.6);
        c = mix(c, srgb!(0x77705f), plain * 0.6);
        c = mix(c, srgb!(0x7d725f), cal * 0.7);
        c = scale(c, 1.0 + 0.4 * cr.bright);
        let mut height = cr.height - 0.001 * cal;
        let mut ray = 0.0;
        for r in &self.rays {
            let (b, h) = r.sample(d, &self.n2);
            ray += b;
            height += h;
        }
        c = scale(c, 1.0 + 0.5 * ray.min(1.5));
        Paint {
            color: scale(c, 1.0 + 0.04 * self.n.at(d * 150.0)),
            height,
            ..Default::default()
        }
    }

    fn relief(&self) -> bool {
        true
    }
}

// --------------------------------------------------------------------------------------- Mars

/// Mars: butterscotch dust with the dark albedo features in their real places (Syrtis Major,
/// Acidalia, Erythraeum, the southern dark belt), the Tharsis volcanoes and Olympus Mons, the
/// scar of Valles Marineris, the bright basins Hellas and Argyre, and white polar caps.
pub(crate) struct Mars {
    n: Noise,
    n2: Noise,
    /// Dark regions, bright regions, mottling, dust streaks, north and south cap edge noise.
    coarse: Grid<6>,
    craters: CraterMap,
    /// Center, radius (radians), relative height, cosine of the reach.
    volcanoes: Vec<(Vec3, f32, f32, f32)>,
    valles: Vec<(f32, f32, f32)>,
}

impl Mars {
    pub(crate) fn new(seed: u64, width: u32) -> Self {
        let dark = vec![
            Region::new(9.0, 69.0, 10.0, 0.65, 0.35, 0.95),
            Region::new(-3.0, 74.0, 8.0, 0.9, 0.35, 0.8),
            Region::new(-8.0, 25.0, 6.0, 3.0, 0.4, 0.85),
            Region::new(-3.0, 0.0, 4.5, 2.2, 0.4, 0.9),
            Region::new(-24.0, -35.0, 11.0, 2.0, 0.45, 0.75),
            Region::new(-10.0, -24.0, 5.0, 1.0, 0.4, 0.7),
            Region::new(-14.0, -50.0, 4.5, 1.4, 0.4, 0.6),
            Region::new(-26.0, -87.0, 3.5, 1.3, 0.3, 0.8),
            Region::new(-30.0, -155.0, 9.0, 2.4, 0.4, 0.75),
            Region::new(-21.0, 143.0, 9.0, 2.6, 0.4, 0.8),
            Region::new(-17.0, 108.0, 8.0, 1.8, 0.4, 0.75),
            Region::new(-30.0, 60.0, 6.0, 2.0, 0.45, 0.55),
            Region::new(48.0, -28.0, 9.0, 1.7, 0.4, 0.85),
            Region::new(45.0, 112.0, 10.0, 2.0, 0.45, 0.5),
            Region::new(72.0, 0.0, 6.0, 30.0, 0.5, 0.45),
            Region::new(-44.0, 0.0, 6.0, 14.0, 0.55, 0.35),
        ];
        let bright = vec![
            Region::new(-42.0, 70.0, 13.0, 1.2, 0.12, 0.9),
            Region::new(-50.0, -43.0, 7.5, 1.1, 0.12, 0.7),
            Region::new(22.0, 20.0, 14.0, 1.5, 0.3, 0.5),
            Region::new(25.0, 147.0, 10.0, 1.2, 0.3, 0.45),
            Region::new(0.0, -160.0, 14.0, 1.8, 0.3, 0.5),
            Region::new(5.0, -110.0, 18.0, 1.0, 0.3, 0.45),
        ];
        let volcanoes = [
            (18.65, -133.8, 4.8, 1.0),
            (11.8, -104.5, 3.0, 0.75),
            (1.5, -112.9, 2.7, 0.7),
            (-8.3, -120.1, 3.2, 0.75),
            (25.0, 147.2, 2.4, 0.5),
            (40.5, -109.6, 8.0, 0.25),
        ]
        .iter()
        .map(|&(lat, lon, r, h)| (dir(lat * DEG, lon * DEG), r * DEG, h, (r * DEG * 2.2).cos()))
        .collect();
        // Valles Marineris from Noctis Labyrinthus east to the chaos regions (lat, lon, width).
        let valles = vec![
            (-6.5, -103.0, 0.9),
            (-7.0, -96.0, 1.1),
            (-7.5, -88.0, 1.5),
            (-7.5, -78.0, 2.4),
            (-9.0, -71.0, 2.2),
            (-12.5, -64.0, 1.4),
            (-13.0, -56.0, 1.3),
            (-12.0, -48.0, 1.8),
            (-8.0, -42.0, 2.2),
            (-3.0, -38.0, 1.6),
        ];
        let n = Noise::new(seed);
        let n2 = Noise::new(seed ^ 0x6d);
        let gw = grid_width(width);
        let coarse = Grid::bake(gw, gw / 2, |d| {
            let mut dk = 0.0f32;
            for r in &dark {
                dk = union(dk, r.cover(d, &n, 0.12));
            }
            let mut br = 0.0f32;
            for r in &bright {
                br = union(br, r.cover(d, &n2, 0.2));
            }
            [
                dk,
                br,
                n.fbm(d * 5.0, 4, 0.6),
                n2.fbm(Vec3::new(d.x * 3.0, d.y, d.z * 3.0) * 8.0, 4, 0.55),
                n.fbm(d * 6.0, 4, 0.6),
                n2.fbm(d * 8.0, 3, 0.6),
            ]
        });
        // Highlands in the south are cratered; the northern plains much less.
        let craters = Craters::new(16.0 * DEG, 0.6, 0.03, width, seed as u32).bake(width, |c| {
            let (lat, lon) = super::noise::lat_lon(c);
            0.25 + 0.75 * dichotomy(lat / DEG, lon)
        });
        Self {
            n,
            n2,
            coarse,
            craters,
            volcanoes,
            valles,
        }
    }

    /// Distance (degrees) from (lat, lon) to the canyon's centerline, and its width there.
    fn valles(&self, lat_deg: f32, lon_deg: f32) -> (f32, f32) {
        let mut best = (f32::MAX, 1.0);
        let cl = (lat_deg * DEG).cos();
        for w in self.valles.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (ax, ay) = (a.1 * cl, a.0);
            let (bx, by) = (b.1 * cl, b.0);
            let (px, py) = (lon_deg * cl, lat_deg);
            let (vx, vy) = (bx - ax, by - ay);
            let t = (((px - ax) * vx + (py - ay) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
            let (qx, qy) = (ax + vx * t - px, ay + vy * t - py);
            let dist = (qx * qx + qy * qy).sqrt();
            if dist < best.0 {
                best = (dist, a.2 + (b.2 - a.2) * t);
            }
        }
        best
    }
}

/// 1 in the cratered southern highlands, 0 in the smooth northern plains.
#[inline]
fn dichotomy(lat_deg: f32, lon: f32) -> f32 {
    smooth(10.0, -15.0, lat_deg + 12.0 * (lon - 1.0).sin())
}

impl Painter for Mars {
    fn paint(&self, t: &Texel) -> Paint {
        let d = t.d;
        let (lat_deg, lon_deg) = (t.lat / DEG, t.lon / DEG);
        let [dark, bright, mottle, streak, north_edge, south_edge] = self.coarse.at(t.lat, t.lon);
        let fine = self.n2.fbm(d * 40.0, 3, 0.55);
        let (dark, bright) = (ragged(dark, fine, 0.5), ragged(bright, fine, 0.3));
        // Dust streaks and mottling break up the dark regions.
        let mottle = mottle + 0.3 * fine;
        let dark = (dark * (0.85 + 0.35 * mottle) - 0.25 * bright).clamp(0.0, 1.0);
        let dust = mix(
            srgb!(0xb57a51),
            srgb!(0xc58d5f),
            smooth(-0.4, 0.6, mottle + 0.3 * bright),
        );
        let rock = mix(srgb!(0x6f4c3a), srgb!(0x5c4337), smooth(-0.3, 0.5, streak));
        let mut c = mix(dust, rock, dark);
        c = scale(c, 1.0 + 0.07 * streak + 0.12 * bright);

        let cr = self.craters.at(t);
        let mut height = cr.height * 0.6;
        c = scale(c, 1.0 + 0.15 * cr.bright);

        // Volcanoes: broad cones with dark summit calderas and lighter flanks.
        for &(center, r, h, cos_reach) in &self.volcanoes {
            if d.dot(center) < cos_reach {
                continue;
            }
            let a = angle_between(d, center) / r;
            if a < 2.2 {
                let cone = (1.0 - a.min(1.0)).powf(1.4);
                let skirt = (-(a - 1.0).max(0.0) * 4.0).exp() * 0.15;
                height += 0.006 * h * (cone + skirt);
                let caldera = smooth(0.16, 0.08, a);
                height -= 0.0008 * h * caldera;
                c = mix(c, scale(c, 0.82), smooth(1.0, 0.4, a) * 0.6 * h);
                c = mix(c, srgb!(0x4e3c34), caldera * 0.8);
                c = mix(
                    c,
                    scale(c, 1.12),
                    smooth(1.25, 1.0, a) * smooth(0.85, 1.0, a),
                );
            }
        }

        // Valles Marineris.
        if (-20.0..5.0).contains(&lat_deg) && (-110.0..-30.0).contains(&lon_deg) {
            let (dist, width) = self.valles(lat_deg, lon_deg);
            let wobble = 1.0 + 0.4 * self.n.at(d * 30.0);
            let w = width * wobble;
            let canyon = smooth(w * 0.55, w * 0.2, dist);
            let walls = smooth(w * 0.9, w * 0.5, dist) * (1.0 - canyon);
            height -= 0.0022 * canyon;
            c = mix(c, srgb!(0x5b3e31), canyon * 0.85);
            c = mix(c, scale(c, 1.15), walls * 0.5);
        }

        // Polar caps: the north cap with dark spiral troughs, the south cap off the pole.
        let north = 80.5 + 2.5 * north_edge + 0.6 * fine;
        let troughs = (t.lon * 1.0 + (90.0 - lat_deg) * 0.55).sin();
        let mut ice = smooth(north - 1.0, north + 0.5, lat_deg) * (0.82 + 0.18 * troughs.abs());
        // Chasma Boreale cuts into the north cap.
        let chasma =
            (-((wrap_pi(t.lon - 0.0 * DEG)) / 0.15).powi(2)).exp() * smooth(88.0, 83.0, lat_deg);
        ice *= 1.0 - 0.7 * chasma;
        let south_c = dir(-87.0 * DEG, -45.0 * DEG);
        let sa = angle_between(d, south_c) / DEG + 1.2 * south_edge + 0.3 * fine;
        ice = ice.max(smooth(5.5, 3.5, sa));
        c = mix(c, srgb!(0xf3efe9), ice);
        c = mix(
            c,
            mix(c, srgb!(0xd8c6b4), 0.5),
            smooth(70.0, 77.0, lat_deg.abs()) * (1.0 - ice),
        );
        Paint {
            color: mul(c, [1.0, 1.0, 1.0]),
            height: height + 0.0006 * mottle,
            ..Default::default()
        }
    }

    fn relief(&self) -> bool {
        true
    }
}

// -------------------------------------------------------------------------------------- Rocky

/// A generic cratered body in its color (asteroid, moon, airless planet).
pub(crate) struct Rocky {
    base: Rgb,
    n: Noise,
    /// Plains, coarse mottling.
    coarse: Grid<2>,
    craters: CraterMap,
}

impl Rocky {
    pub(crate) fn new(color: [f32; 3], seed: u64, width: u32) -> Self {
        let mut rng = Rng::new(seed);
        let plains = (0..5)
            .map(|_| {
                Region::new(
                    rng.range(-60.0, 60.0),
                    rng.range(-180.0, 180.0),
                    rng.range(6.0, 16.0),
                    rng.range(0.8, 1.6),
                    0.35,
                    rng.range(0.4, 0.8),
                )
            })
            .collect();
        let plains: Vec<Region> = plains;
        let n = Noise::new(seed);
        let gw = grid_width(width);
        let coarse = Grid::bake(gw, gw / 2, |d| {
            let mut plain = 0.0f32;
            for r in &plains {
                plain = union(plain, r.cover(d, &n, 0.1));
            }
            [plain, n.fbm(d * 6.0, 3, 0.6)]
        });
        let craters = Craters::new(16.0 * DEG, 0.6, 0.05, width, seed as u32)
            .bake(width, |c| 1.0 - 0.6 * coarse.at_dir(c)[0]);
        Self {
            base: color,
            n,
            coarse,
            craters,
        }
    }
}

impl Painter for Rocky {
    fn paint(&self, t: &Texel) -> Paint {
        let d = t.d;
        let [plain, mottle] = self.coarse.at(t.lat, t.lon);
        let fine = self.n.fbm(d * 48.0, 2, 0.5);
        let plain = ragged(plain, fine, 0.5);
        let cr = self.craters.at(t);
        let mottle = mottle + 0.35 * fine;
        let mut c = scale(self.base, (0.85 + 0.12 * mottle) * (1.0 - 0.35 * plain));
        c = scale(c, 1.0 + 0.4 * cr.bright);
        Paint {
            color: c,
            height: cr.height - 0.001 * plain,
            ..Default::default()
        }
    }

    fn relief(&self) -> bool {
        true
    }
}
