//! Flow for the gas giants: zonal jets plus eddies and vortices, run backward for a few days
//! so every point of the map knows where its cloud came from.
//!
//! Painting a band profile and noise at those source points does what the real atmosphere
//! does: the jets shear everything into streaks, eddies curl band edges into filaments and
//! vortices wind their insides into spirals. The flow is solved on a coarse grid (it is smooth
//! even where the pattern it makes is not) and sampled per texel.
//!
//! Units: angles in radians, time in days. A stream function `psi` (radians² per day) gives
//! the eddy velocity: east = −∂ψ/∂y, north = ∂ψ/∂x, with x east and y north in radians of arc.

use super::noise::{Grid, Noise, uv};
use engine_core::math::Vec3;

/// Days × (radians of arc per day) per m/s on a body of `radius_m`.
pub(crate) fn mps(speed: f32, radius_m: f32) -> f32 {
    speed * 86_400.0 / radius_m
}

/// Where the material at each point was `days` ago.
pub(crate) struct Flow {
    /// Source latitude, source longitude minus longitude (unwrapped).
    src: Grid<2>,
}

/// A vortex in the stream function: a Gaussian bump of ψ with an elliptical footprint.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Swirl {
    pub lat: f32,
    pub lon: f32,
    /// North-south radius (radians) of the visible oval.
    pub radius: f32,
    /// East-west radius over north-south radius.
    pub stretch: f32,
    /// Peak wind around it (radians per day); positive turns counterclockwise seen from
    /// outside (cyclonic in the north, anticyclonic in the south).
    pub speed: f32,
}

impl Swirl {
    /// Elliptical radius (1 = the oval's edge) and the east/north offsets in units of the
    /// radius.
    #[inline]
    pub fn rho(&self, lat: f32, lon: f32) -> (f32, f32, f32) {
        let dx =
            super::noise::wrap_pi(lon - self.lon) * self.lat.cos() / (self.radius * self.stretch);
        let dy = (lat - self.lat) / self.radius;
        ((dx * dx + dy * dy).sqrt(), dx, dy)
    }

    /// Its stream function at (lat, lon).
    #[inline]
    pub fn psi(&self, lat: f32, lon: f32) -> f32 {
        // The Gaussian is a bit wider than the oval so the fastest wind runs along its edge.
        let (rho, ..) = self.rho(lat, lon);
        let g = rho / 1.15;
        if g > 4.0 {
            return 0.0;
        }
        // Peak speed of S·exp(−r²/a²) is 0.86·S/a (at r = a/√2).
        let a = self.radius * 1.15;
        -self.speed * a / 0.86 * (-g * g).exp()
    }
}

/// Coherent eddy turbulence: fractal noise as a stream function, so its wind is the noise's
/// gradient turned 90°. `freq` is the largest eddies' frequency (per radian); the result's
/// typical wind is about `speed` (radians per day) on every scale.
#[inline]
pub(crate) fn eddies(n: &Noise, d: Vec3, freq: f32, octaves: u32, speed: f32) -> f32 {
    n.fbm(d * freq, octaves, 0.5) * speed / freq * 0.6
}

impl Flow {
    /// Solve the flow on a grid `width` cells around: `jet(lat)` is the zonal wind (radians of
    /// arc per day, east positive), `psi(direction, lat, lon)` the eddies' stream function.
    /// Runs `days` backward in `steps` midpoint steps.
    pub(crate) fn new(
        width: usize,
        jet: impl Fn(f32) -> f32 + Sync,
        psi: impl Fn(Vec3, f32, f32) -> f32 + Sync,
        days: f32,
        steps: u32,
    ) -> Self {
        use std::f32::consts::{FRAC_PI_2, PI, TAU};
        // Stream function and velocity on a grid half the size: the eddies are smooth.
        let pw = (width / 2).max(32);
        let ph = pw / 2;
        let psi_grid = Grid::<1>::bake_lat_lon(pw, ph, |d, lat, lon| [psi(d, lat, lon)]);
        let dlon = TAU / pw as f32;
        let dlat = PI / ph as f32;
        let at = |x: isize, y: isize| -> f32 {
            let x = x.rem_euclid(pw as isize) as usize;
            let y = y.clamp(0, ph as isize - 1) as usize;
            psi_grid.data[y * pw + x][0]
        };
        let mut vel = Grid::<2> {
            width: pw,
            height: ph,
            data: vec![[0.0; 2]; pw * ph],
        };
        super::bake::par_rows(&mut vel.data, pw, |y, row| {
            let lat = FRAC_PI_2 - (y as f32 + 0.5) * dlat;
            let cos = lat.cos().max(0.12);
            let u = jet(lat);
            let (y, ny) = (y as isize, ph as isize);
            for (x, v) in row.iter_mut().enumerate() {
                let x = x as isize;
                let dpsi_dlon = (at(x + 1, y) - at(x - 1, y)) / (2.0 * dlon * cos);
                let (n, s) = ((y - 1).max(0), (y + 1).min(ny - 1));
                let dpsi_dlat = (at(x, n) - at(x, s)) / ((s - n) as f32 * dlat);
                // North wind, and east wind as longitude per day.
                *v = [dpsi_dlon, (u - dpsi_dlat) / cos];
            }
        });
        let dt = days / steps.max(1) as f32;
        let limit = FRAC_PI_2 - 0.02;
        let src = Grid::<2>::bake_lat_lon(width, (width / 2).max(1), |_, lat0, lon0| {
            let (mut lat, mut lon) = (lat0, lon0);
            for _ in 0..steps {
                let v1 = vel.at(lat, lon);
                let mlat = (lat - v1[0] * dt * 0.5).clamp(-limit, limit);
                let mlon = lon - v1[1] * dt * 0.5;
                let v2 = vel.at(mlat, mlon);
                lat = (lat - v2[0] * dt).clamp(-limit, limit);
                lon -= v2[1] * dt;
            }
            [lat, lon - lon0]
        });
        Self { src }
    }

    /// Where the material at (lat, lon) came from: (latitude, longitude), longitude unwrapped.
    #[inline]
    pub(crate) fn source(&self, lat: f32, lon: f32) -> (f32, f32) {
        let (u, v) = uv(lat, lon);
        let [slat, dlon] = self.src.sample(u, v);
        (slat, lon + dlon)
    }
}

/// A zonal wind profile from (latitude°, m/s) stops, south to north, as radians per day.
pub(crate) fn jet_profile(stops: &[(f32, f32)], lat: f32, radius_m: f32) -> f32 {
    let lat_deg = lat.to_degrees();
    let mut v = stops.first().map(|s| s.1).unwrap_or(0.0);
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if lat_deg >= a.0 && lat_deg <= b.0 {
            let t = (lat_deg - a.0) / (b.0 - a.0).max(1e-3);
            let t = t * t * (3.0 - 2.0 * t);
            v = a.1 + (b.1 - a.1) * t;
            break;
        }
        if lat_deg > b.0 {
            v = b.1;
        }
    }
    mps(v, radius_m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jets_shear_and_vortices_turn() {
        // A pure jet moves material east: it came from the west.
        let flow = Flow::new(64, |_| 0.2, |_, _, _| 0.0, 1.0, 8);
        let (lat, lon) = flow.source(0.1, 0.5);
        assert!((lat - 0.1).abs() < 1e-3);
        assert!((lon - (0.5 - 0.2 / 0.1f32.cos())).abs() < 0.01, "{lon}");
        // A counterclockwise vortex: north of its center the wind blows west, so material there
        // came from the east.
        let s = Swirl {
            lat: 0.0,
            lon: 0.0,
            radius: 0.2,
            stretch: 1.0,
            speed: 0.1,
        };
        let flow = Flow::new(256, |_| 0.0, |_, lat, lon| s.psi(lat, lon), 0.5, 8);
        let (_, lon) = flow.source(0.12, 0.0);
        assert!(lon > 0.01, "{lon}");
        let (_, lon) = flow.source(-0.12, 0.0);
        assert!(lon < -0.01, "{lon}");
    }
}
