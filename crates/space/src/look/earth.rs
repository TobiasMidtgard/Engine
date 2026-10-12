//! Earth: continents and oceans with climate-driven biomes (rainforest, savanna, desert,
//! temperate and boreal forest, tundra, ice sheets), mountain ranges with snow, shallow seas
//! along the coasts, an ice-covered polar continent in the south and sea ice around both
//! poles; and a cloud layer with the real circulation in it: the convective band near the
//! equator, clear subtropics, mid-latitude storm tracks whose cyclones wind clouds into commas
//! with long trailing fronts, and a few tropical cyclones.
//!
//! Smooth fields (continent shapes, climate noise, the large cloud systems) are computed on
//! coarse grids; only the fine detail is evaluated per texel.

use std::f32::consts::PI;

use engine_core::math::{Rng, Vec3};

use super::bake::{Paint, Painter, Texel};
use super::color::{Rgb, mix, scale, srgb};
use super::noise::{Grid, Noise, angle_between, dir, local_offset, smooth};

const DEG: f32 = PI / 180.0;

#[derive(Debug, Clone, Copy)]
struct Storm {
    center: Vec3,
    lat: f32,
    /// Radius, radians.
    radius: f32,
    /// Cosine of the storm's reach (2.2 radii): a quick test before the real work.
    cos_reach: f32,
    /// Winding of the spiral (radians per e-fold of radius); sign = sense of rotation.
    wind: f32,
    tropical: bool,
}

pub(crate) struct Earth {
    n: Noise,
    n2: Noise,
    nc: Noise,
    nc2: Noise,
    /// x, y, z: warped position for the continents; w: their low-frequency value.
    land: Grid<4>,
    /// Climate noise: moisture wobble, mountain-range mask, sea-ice edge.
    climate: Grid<3>,
    /// Large cloud systems (unswirled): body, cirrus.
    sky: Grid<2>,
    storms: Vec<Storm>,
}

/// Land where the continent field is above this.
const SEA_LEVEL: f32 = 0.1;

impl Earth {
    pub(crate) fn new(seed: u64, width: u32) -> Self {
        let n = Noise::new(seed);
        let n2 = Noise::new(seed ^ 0x2b);
        let nc = Noise::new(seed ^ 0xc1);
        let nc2 = Noise::new(seed ^ 0xc2);
        let wn = Noise::new(seed ^ 0x1a);
        let gw = (width as usize / 4).clamp(64, 512);
        let land = Grid::bake_lat_lon(gw, gw / 2, |d, lat, _| {
            let w = Vec3::new(
                wn.fbm(d * 1.3 + Vec3::new(1.0, 0.0, 0.0), 4, 0.5),
                wn.fbm(d * 1.3 + Vec3::new(0.0, 7.0, 0.0), 4, 0.5),
                wn.fbm(d * 1.3 + Vec3::new(0.0, 0.0, 3.0), 4, 0.5),
            );
            let p = d * 1.15 + w * 0.42;
            let lat_deg = lat / DEG;
            // A continent on the south pole, an ocean on the north pole, as on Earth.
            let v = n.fbm(p, 4, 0.55) + 0.45 * smooth(-58.0, -72.0, lat_deg)
                - 0.35 * smooth(70.0, 82.0, lat_deg);
            [p.x, p.y, p.z, v]
        });
        let climate = Grid::bake(gw, gw / 2, |d| {
            let [x, y, z, _] = land.at_dir(d);
            let p = Vec3::new(x, y, z);
            [
                n2.fbm(d * 4.0 + Vec3::splat(2.0), 4, 0.55),
                smooth(0.0, 0.5, n2.fbm(p * 1.4 + Vec3::splat(9.0), 3, 0.5)),
                n2.fbm(d * 5.0, 4, 0.5),
            ]
        });
        let sky = Grid::bake_lat_lon(gw, gw / 2, |d, lat, _| {
            // Jet-stream shear stretches mid-latitude clouds east-west.
            let shear = 1.0 + 1.2 * smooth(22.0, 45.0, lat.abs() / DEG);
            let q = Vec3::new(d.x, d.y * shear, d.z);
            let warp = nc2.fbm(q * 3.0, 3, 0.5);
            [
                nc.fbm(q * 5.0 + Vec3::splat(warp * 0.7), 4, 0.6),
                smooth(
                    0.15,
                    0.6,
                    nc2.fbm(Vec3::new(q.x, q.y * 3.0, q.z) * 7.0, 4, 0.55),
                ),
            ]
        });
        let mut rng = Rng::new(seed ^ 0x57);
        let mut storms = Vec::new();
        for hemi in [1.0f32, -1.0] {
            let count = 6;
            for k in 0..count {
                let lat = rng.range(40.0, 60.0) * hemi;
                let lon =
                    -PI + (k as f32 + rng.range(0.1, 0.9)) * std::f32::consts::TAU / count as f32;
                let radius = rng.range(10.0, 15.0) * DEG;
                storms.push(Storm {
                    center: dir(lat * DEG, lon),
                    lat: lat * DEG,
                    radius,
                    cos_reach: (radius * 2.2).cos(),
                    wind: 1.0 * hemi * rng.range(0.8, 1.2),
                    tropical: false,
                });
            }
        }
        for (lat, lon) in [(16.0, -60.0), (-17.0, 75.0), (21.0, 140.0)] {
            let lat = lat + rng.range(-3.0, 3.0);
            let radius = rng.range(6.0, 8.0) * DEG;
            storms.push(Storm {
                center: dir(lat * DEG, (lon + rng.range(-10.0, 10.0)) * DEG),
                lat: lat * DEG,
                radius,
                cos_reach: (radius * 2.2).cos(),
                wind: 2.2 * lat.signum(),
                tropical: true,
            });
        }
        Self {
            n,
            n2,
            nc,
            nc2,
            land,
            climate,
            sky,
            storms,
        }
    }

    /// Continent field (land above `SEA_LEVEL`), with coastline detail down to the texel.
    #[inline]
    fn continent(&self, t: &Texel) -> (f32, Vec3) {
        let [x, y, z, low] = self.land.at(t.lat, t.lon);
        let p = Vec3::new(x, y, z);
        // Finer octaves for the coastlines (on the sphere itself, so they don't streak where
        // the coarse warp stretches).
        let fine = self.n.fbm(t.d * 18.0 + Vec3::splat(3.3), 4, 0.5);
        (low + 0.11 * fine, p)
    }

    fn surface(&self, t: &Texel, c: f32, p: Vec3, climate: [f32; 3]) -> (Rgb, f32) {
        let [wobble, ranges, ice_noise] = climate;
        let lat_deg = t.lat / DEG;
        let lat = lat_deg.abs();
        let coast = c - SEA_LEVEL;
        if coast < 0.0 {
            // Ocean: deep navy, lighter shelves along the coasts, sea ice near the poles.
            let shelf = smooth(-0.05, 0.0, coast);
            let tropical = smooth(35.0, 15.0, lat);
            let shallow = mix(srgb!(0x18476a), srgb!(0x226f84), tropical * 0.6);
            let deep = mix(srgb!(0x0b1f45), srgb!(0x091a3b), smooth(-0.1, -0.4, coast));
            let mut col = mix(deep, shallow, shelf * shelf * 0.8);
            let edge = if lat_deg > 0.0 { 77.0 } else { 66.0 } + 4.0 * ice_noise;
            let ice = smooth(edge - 1.5, edge + 1.5, lat);
            let leads = 0.85 + 0.15 * self.n2.at(t.d * 80.0);
            col = mix(col, scale(srgb!(0xdde4ea), leads), ice);
            return (col, 1.0 - ice);
        }
        // Land: elevation, temperature and moisture choose the biome.
        let ridge = self.n2.ridged(p * 3.5, 4);
        let elev = (coast * 2.5).min(0.5) + ridge * ranges * 0.8;
        // Northern summer: the tundra is bare, snow lies only on the high ranges.
        let temp = 1.0 - smooth(0.0, 88.0, lat) - elev * 0.35;
        let band = if lat < 12.0 {
            0.85
        } else if lat < 35.0 {
            0.85 - 0.65 * smooth(12.0, 22.0, lat) + 0.4 * smooth(28.0, 38.0, lat)
        } else {
            0.6 - 0.25 * smooth(60.0, 75.0, lat)
        };
        let interior = smooth(0.02, 0.22, coast);
        let moist = (band - 0.35 * interior + 0.3 * wobble).clamp(0.0, 1.0);
        let forest = if temp > 0.62 {
            srgb!(0x22401b)
        } else if temp > 0.4 {
            srgb!(0x30502a)
        } else {
            srgb!(0x283c2b)
        };
        let grass = mix(srgb!(0x8a7f52), srgb!(0x6a7043), smooth(0.3, 0.6, temp));
        // Texture down to the texel: dune seas, darker rock, dry valleys.
        let grain = self.n.fbm(t.d * 70.0, 3, 0.55);
        let tone = self.n.fbm(p * 6.0, 2, 0.5) + 0.35 * grain;
        let mut sand = mix(
            mix(srgb!(0xd6c29c), srgb!(0xcda97c), smooth(-0.4, 0.2, tone)),
            srgb!(0xb4865a),
            smooth(0.15, 0.6, tone),
        );
        sand = mix(
            sand,
            srgb!(0x7d6550),
            smooth(0.25, 0.7, ridge * ranges + 0.3 * grain) * 0.6,
        );
        let tundra = srgb!(0x706d5c);
        let mut col = if moist > 0.55 {
            mix(grass, forest, smooth(0.55, 0.7, moist))
        } else {
            mix(sand, grass, smooth(0.22, 0.45, moist))
        };
        col = mix(
            col,
            tundra,
            smooth(0.32, 0.18, temp) * (1.0 - smooth(0.0, 0.5, elev - 0.6)),
        );
        // Bare rock and snow on the ranges.
        col = mix(col, srgb!(0x6c6253), smooth(0.45, 0.85, elev) * 0.7);
        let snow = smooth(0.05, -0.05, temp + 0.06 * wobble)
            + smooth(1.0, 1.2, elev) * smooth(0.45, 0.25, temp);
        col = mix(col, srgb!(0xf0f2f4), snow.min(1.0));
        // Ice sheets: the polar continent, and the highest, coldest northern land.
        let sheet = if lat_deg < 0.0 {
            smooth(58.0, 63.0, lat + 3.0 * wobble)
        } else {
            smooth(74.0, 79.0, lat + 3.0 * wobble + 3.0 * elev)
        };
        col = mix(col, srgb!(0xf2f5f8), sheet);
        col = scale(col, 1.0 + 0.1 * grain);
        (col, 0.0)
    }

    fn cloud_cover(&self, t: &Texel, desert: f32) -> f32 {
        let lat_deg = t.lat / DEG;
        let a = lat_deg.abs();
        // Climatology: wet near the equator, clear subtropics, stormy mid-latitudes, the
        // Southern Ocean overcast, the ice sheet below clear.
        let itcz = (-((lat_deg - 5.0) / 8.0).powi(2)).exp();
        let mut cover = 0.45 + 0.26 * itcz - 0.16 * smooth(12.0, 22.0, a) * smooth(36.0, 28.0, a)
            + 0.3 * smooth(35.0, 50.0, a)
            + 0.12 * smooth(-45.0, -55.0, lat_deg) * smooth(-70.0, -62.0, lat_deg)
            - 0.25 * smooth(-66.0, -76.0, lat_deg)
            - 0.08 * smooth(72.0, 85.0, lat_deg);
        cover -= 0.3 * desert;
        // Swirl the sampling point around storms (one after another where they overlap).
        let mut q = t.d;
        let mut swirled = false;
        let mut front = 0.0f32;
        let mut eye = 1.0f32;
        let mut wall = 0.0f32;
        for s in &self.storms {
            if q.dot(s.center) < s.cos_reach {
                continue;
            }
            let ang = angle_between(q, s.center);
            let r = (ang / s.radius).max(1e-3);
            let (e, n) = local_offset(q, s.center);
            let bearing = n.atan2(e);
            let turn = s.wind * (1.0 / r.max(0.08)).ln().max(0.0) * smooth(2.2, 0.8, r);
            let (sn, cs) = turn.sin_cos();
            let (re, rn) = (e * cs - n * sn, e * sn + n * cs);
            let east = Vec3::Y.cross(s.center).try_normalize().unwrap_or(Vec3::X);
            let north = s.center.cross(east);
            q = (s.center + east * re + north * rn).normalize();
            swirled = true;
            if s.tropical {
                let arms = 0.5 + 0.5 * (2.0 * (bearing + s.wind * r.ln())).cos();
                cover += 0.55 * smooth(1.3, 0.3, r) * (0.4 + 0.6 * arms);
                wall += 0.6 * smooth(0.25, 0.1, r) * smooth(0.03, 0.06, r);
                eye = eye.min(0.15 + 0.85 * smooth(0.02, 0.045, r));
            } else {
                // A comma: the cloud head on the poleward side, a dry slot wrapping in from
                // the equatorward west, and the cold front trailing toward the equator.
                // Bearings are mirrored in the south so "poleward" is always +π/2.
                let bp = (n * s.lat.signum()).atan2(e);
                let head = 0.5 + 0.5 * bp.sin();
                cover += 0.3 * head * smooth(1.4, 0.3, r);
                let slot_at = -2.3 - 0.9 * r.ln();
                let slot = (-(super::noise::wrap_pi(bp - slot_at) / 0.5).powi(2)).exp()
                    * smooth(1.5, 0.5, r)
                    * smooth(0.04, 0.15, r);
                cover -= 0.5 * slot;
                let tail_at = -1.35 - 0.55 * (r - 0.3).max(0.0);
                let arm = (-(super::noise::wrap_pi(bp - tail_at) / 0.2).powi(2)).exp();
                front = front.max(arm * smooth(2.6, 0.9, r) * smooth(0.15, 0.45, r));
            }
        }
        let [body, cirrus] = if swirled {
            self.sky.at_dir(q)
        } else {
            self.sky.at(t.lat, t.lon)
        };
        // Finer detail: billows and the speckle of small cumulus.
        let shear = 1.0 + 1.2 * smooth(22.0, 45.0, a);
        let qs = Vec3::new(q.x, q.y * shear, q.z);
        let fine = self.nc.fbm(qs * 40.0 + Vec3::splat(1.7), 3, 0.6);
        let puffs = 1.0 - self.nc2.at(q * 110.0).abs() * 1.6;
        let tropic_puff = smooth(32.0, 10.0, a);
        let field = body + 0.22 * fine + 0.12 * puffs * tropic_puff + 0.5 * front;
        let threshold = 0.62 - cover * 1.05;
        let mut c = smooth(threshold, threshold + 0.38, field);
        // Thin cirrus veils in the storm belts.
        c = c.max(cirrus * 0.35 * smooth(25.0, 45.0, a) * (0.7 + 0.3 * fine));
        c = (c + wall).min(1.0) * eye;
        c.clamp(0.0, 1.0)
    }
}

impl Painter for Earth {
    fn paint(&self, t: &Texel) -> Paint {
        let (c, p) = self.continent(t);
        let climate = self.climate.at(t.lat, t.lon);
        let (color, water) = self.surface(t, c, p, climate);
        let lat = t.lat.abs() / DEG;
        let dry_band = smooth(14.0, 20.0, lat) * smooth(34.0, 26.0, lat);
        let desert = if water > 0.5 {
            0.0
        } else {
            dry_band * smooth(0.0, 0.08, c - SEA_LEVEL)
        };
        Paint {
            color,
            water,
            height: 0.0,
            cloud: self.cloud_cover(t, desert),
        }
    }

    fn clouds(&self) -> bool {
        true
    }
}
