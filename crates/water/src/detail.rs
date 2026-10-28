//! Fine wind waves: the short end of a body's wave spectrum, from where its Gerstner waves stop
//! (about a fifth of the peak wavelength, never under 12 cm) down to 2 cm ripples.
//!
//! There are far too many of these for a sum of waves, so the renderer evolves them as two
//! periodic tiles by FFT (Tessendorf, "Simulating Ocean Water", 2001), the way ocean renderers
//! from *Assassin's Creed* to *Sea of Thieves* and Unreal's water do: each frame it turns this
//! module's spectrum amplitudes into slopes, heights and the surface's squeeze (the Jacobian of
//! its horizontal chop) on the GPU. The tiles differ in size by an irrational ratio, so their
//! repeats never line up. The spectrum continues the body's own JONSWAP spectrum at the same
//! level ([`crate::waves::jonswap_shape`]), so the detail matches the waves it rides on, and
//! wave heights the physics sees stay the Gerstner set's alone: the detail is centimeters.
//!
//! What a pixel can't resolve still matters: at a distance thousands of ripples share a pixel
//! and roughen it. Their slope variance (Cox and Munk's mean square slope, 1954) widens the
//! sun's glitter and blurs reflections. The tiles keep slope moments in their mips (LEAN
//! mapping, Olano and Baker 2010; Bruneton et al. 2010 for the ocean), and what lies beyond the
//! finer tile comes from [`residual_slope_variance`].

use engine_core::math::Vec2;

use crate::waves::{G, jonswap_shape, sea_state, wind_band};

/// Texels along a tile's side.
pub const N: usize = 128;
/// Tiles: one for the longer detail waves, one for the ripples.
pub const CASCADES: usize = 2;
/// The finer tile is this many times smaller than the coarser one: no ratio of small whole
/// numbers, so their repeats never line up.
pub const CASCADE_RATIO: f32 = 7.37;
/// The shortest waves drawn, meters: shorter capillary ripples only roughen the surface.
pub const SHORTEST_M: f32 = 0.02;
/// The tiles repeat in time after this many seconds: frequencies are rounded to whole
/// multiples of 2π over it, so the clock driving them can wrap.
pub const PERIOD_S: f32 = 64.0;
/// Surface tension over density, m³/s²: ripples under 2 cm or so are held by it, not by
/// gravity, and run faster than gravity waves of their length would.
pub const TENSION: f32 = 7.4e-5;
/// Spreading of the detail waves about the wind, `cos²ˢ(θ/2)`: short waves travel in all
/// directions, mostly downwind (Mitsuyasu et al. 1975 find s near 1-3 well above the peak).
const SPREAD_S: f32 = 2.0;
/// Bins the coarser tile keeps: up to this fraction of its Nyquist wavenumber (the rest goes to
/// the finer tile, which resolves it with room to spare).
const SPLIT: f32 = 0.75;

/// Dispersion of short waves in deep water: ω² = g·k + (σ/ρ)·k³.
pub fn omega(k: f32) -> f32 {
    (G * k + TENSION * k * k * k).sqrt()
}

/// What the detail is made from: one body's wind sea.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetailSea {
    /// Wind at 10 m, m/s.
    pub wind: f32,
    /// Direction the wind blows toward (unit, world x/z).
    pub dir: Vec2,
    /// Fetch, meters (0 = open ocean).
    pub fetch_m: f32,
    /// The body's wave height scale.
    pub height_scale: f32,
    /// Crest sharpness, 0..1.
    pub choppiness: f32,
}

/// A wind sea's spectrum: JONSWAP at the level the Gerstner waves hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spectrum {
    /// Scale of [`jonswap_shape`], m²·s⁻⁴.
    pub level: f32,
    /// Peak frequency, rad/s.
    pub wp: f32,
    /// Where the Gerstner waves stop and the detail starts, rad/s.
    pub w_cut: f32,
}

impl Spectrum {
    /// The spectrum of `sea`, or `None` on calm water.
    pub fn of(sea: &DetailSea) -> Option<Self> {
        if sea.wind < 0.2 || sea.height_scale <= 0.0 {
            return None;
        }
        let (hs, wp) = sea_state(sea.wind, sea.fetch_m);
        let hs = hs * sea.height_scale;
        let (w_min, w_max) = wind_band(wp);
        // The Gerstner set holds m0 = (Hs/4)² between w_min and w_max (`build_waves`).
        let steps = 256;
        let ratio = (w_max / w_min).powf(1.0 / steps as f32);
        let mut total = 0.0;
        let mut w = w_min;
        for _ in 0..steps {
            let next = w * ratio;
            total += jonswap_shape((w * next).sqrt(), wp) * (next - w);
            w = next;
        }
        if total <= 0.0 || hs <= 0.0 {
            return None;
        }
        Some(Self {
            level: (hs / 4.0) * (hs / 4.0) / total,
            wp,
            w_cut: w_max,
        })
    }

    /// Variance density at angular frequency `w`, m²·s.
    pub fn at(&self, w: f32) -> f32 {
        self.level * jonswap_shape(w, self.wp)
    }

    /// The wavenumber the detail starts at, rad/m.
    pub fn k_cut(&self) -> f32 {
        self.w_cut * self.w_cut / G
    }

    /// The directional wavenumber spectrum at `k` (rad/m, relative to the wind direction
    /// along +x): variance per (rad/m)², m⁴. Integrates over the plane to the variance.
    pub fn density(&self, k: Vec2) -> f32 {
        let kl = k.length();
        if kl < 1e-6 {
            return 0.0;
        }
        let w = omega(kl);
        let dw_dk = (G + 3.0 * TENSION * kl * kl) / (2.0 * w);
        let theta = k.y.atan2(k.x);
        self.at(w) * dw_dk / kl * spreading(theta)
    }
}

/// `cos²ˢ(θ/2)` normalized over the circle.
fn spreading(theta: f32) -> f32 {
    // ∫cos⁴(θ/2) dθ over the circle is 3π/4.
    debug_assert_eq!(SPREAD_S, 2.0);
    (theta * 0.5).cos().powi(4) / (0.75 * std::f32::consts::PI)
}

/// The two tiles' sizes (meters) for a sea whose detail starts at `k_cut`: the coarser tile
/// four times the longest detail wave (so a tile holds several of them), at least 4 m and on
/// a half-octave step so small wind changes keep the same tiles.
pub fn tile_sizes(k_cut: f32) -> [f32; CASCADES] {
    let longest = std::f32::consts::TAU / k_cut.max(1e-3);
    let l0 = (4.0 * longest).clamp(4.0, 128.0);
    let l0 = 2f32.powf((l0.log2() * 2.0).round() / 2.0);
    [l0, l0 / CASCADE_RATIO]
}

/// The band of wavenumbers (rad/m) tile `c` of `sizes` draws for a sea whose detail starts at
/// `k_cut`.
pub fn band(sizes: [f32; CASCADES], c: usize, k_cut: f32) -> (f32, f32) {
    let nyquist = |l: f32| std::f32::consts::PI * N as f32 / l;
    let k_max = std::f32::consts::TAU / SHORTEST_M;
    let split = SPLIT * nyquist(sizes[0]);
    match c {
        0 => (k_cut, split.min(k_max)),
        _ => (split.max(k_cut), (SPLIT * nyquist(sizes[1])).min(k_max)),
    }
}

/// A standard normal number from a seed and an index (Box–Muller over two hashes).
fn gaussian(seed: u64, i: u64) -> f32 {
    let h = |j: u64| {
        let x = engine_core::math::hash_u64(seed ^ (i * 2 + j).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        ((x >> 40) as f32 + 0.5) / (1u64 << 24) as f32
    };
    let (u1, u2) = (h(0), h(1));
    (-2.0 * u1.ln()).sqrt() * (std::f32::consts::TAU * u2).cos()
}

/// The amplitudes a tile starts from: for each bin (row-major, wavenumber index `n − N/2`
/// along x then z), `h0(k)` and `conj(h0(−k))` as (re, im, re, im). The wind blows toward
/// `dir`. Only the bins in `band` hold any.
pub fn amplitudes(
    spectrum: &Spectrum,
    dir: Vec2,
    size: f32,
    band: (f32, f32),
    seed: u64,
) -> Vec<[f32; 4]> {
    let dk = std::f32::consts::TAU / size;
    let half = (N / 2) as i32;
    let (c, s) = (dir.x, dir.y);
    // h0 for one bin: complex Gaussian with ⟨|h0|²⟩ = E·Δk²/2, so h0(k)·e^{iωt} + conj(h0(−k))·
    // e^{−iωt} carries the bin's variance.
    let h0 = |nx: i32, nz: i32| -> [f32; 2] {
        let k = Vec2::new(nx as f32, nz as f32) * dk;
        let kl = k.length();
        if kl < band.0 || kl >= band.1 {
            return [0.0, 0.0];
        }
        let along = Vec2::new(c * k.x + s * k.y, -s * k.x + c * k.y);
        let amp =
            (spectrum.density(along) * dk * dk * 0.5).sqrt() * std::f32::consts::FRAC_1_SQRT_2;
        // The seed per wavenumber (not per bin index), so ±k draw independent numbers.
        let i = ((nz + 1024) as u64) << 16 | (nx + 1024) as u64;
        [amp * gaussian(seed, i * 2), amp * gaussian(seed, i * 2 + 1)]
    };
    let mut out = Vec::with_capacity(N * N);
    for z in 0..N as i32 {
        for x in 0..N as i32 {
            let (nx, nz) = (x - half, z - half);
            let a = h0(nx, nz);
            let b = h0(-nx, -nz);
            out.push([a[0], a[1], b[0], -b[1]]);
        }
    }
    out
}

/// Mean square slope (both axes) of the spectrum between wavenumbers `lo` and `hi`.
pub fn slope_variance(spectrum: &Spectrum, lo: f32, hi: f32) -> f32 {
    if hi <= lo {
        return 0.0;
    }
    // ∫ k²·S(ω)·dω over the band (spreading integrates to 1).
    let steps = 200;
    let (w_lo, w_hi) = (omega(lo), omega(hi));
    let ratio = (w_hi / w_lo).powf(1.0 / steps as f32);
    let mut sum = 0.0;
    let mut w = w_lo;
    for _ in 0..steps {
        let next = w * ratio;
        let wm = (w * next).sqrt();
        // k from ω (gravity-capillary): a few Newton steps from the gravity guess.
        let mut k = wm * wm / G;
        for _ in 0..6 {
            let f = G * k + TENSION * k * k * k - wm * wm;
            k -= f / (G + 3.0 * TENSION * k * k);
        }
        sum += k * k * spectrum.at(wm) * (next - w);
        w = next;
    }
    sum
}

/// Cox and Munk's (1954) mean square slope of a clean sea surface in wind `u` (m/s at about
/// 12 m): 0.003 + 5.12e-3·U, from photographs of the sun's glitter.
pub fn cox_munk(u: f32) -> f32 {
    0.003 + 5.12e-3 * u.max(0.0)
}

/// The slope variance no wave or tile draws: what the sun's glitter shows of ripples under
/// 2 cm and anything the tiles leave out, so the whole adds up to Cox and Munk's. `drawn` is
/// what the Gerstner waves and the tiles hold at full resolution.
pub fn residual_slope_variance(wind: f32, drawn: f32) -> f32 {
    // Light airs leave sheltered water glassy: the glitter widens from a light breeze on.
    let t = ((wind - 2.0) / 6.0).clamp(0.0, 1.0);
    (cox_munk(wind) - drawn).max(0.0) * t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ocean(wind: f32) -> DetailSea {
        DetailSea {
            wind,
            dir: Vec2::X,
            fetch_m: 0.0,
            height_scale: 1.0,
            choppiness: 0.6,
        }
    }

    #[test]
    fn the_detail_continues_the_gerstner_spectrum() {
        let sea = ocean(10.0);
        let s = Spectrum::of(&sea).unwrap();
        // Integrating the spectrum over the Gerstner band gives the sea state's variance.
        let (hs, wp) = sea_state(10.0, 0.0);
        let (w_min, w_max) = wind_band(wp);
        let mut m0 = 0.0;
        let n = 2000;
        for i in 0..n {
            let w = w_min + (w_max - w_min) * (i as f32 + 0.5) / n as f32;
            m0 += s.at(w) * (w_max - w_min) / n as f32;
        }
        assert!((m0 / (hs * hs / 16.0) - 1.0).abs() < 0.02, "{m0}");
        // Far above the peak it falls as ω⁻⁵ at about the Phillips level (α ≈ 0.008-0.015).
        let w = 8.0 * wp;
        let alpha = s.at(w) * w.powi(5) / (G * G);
        assert!(alpha > 0.005 && alpha < 0.03, "α {alpha}");
    }

    #[test]
    fn the_tiles_hold_the_variance_of_their_bands() {
        let sea = ocean(10.0);
        let s = Spectrum::of(&sea).unwrap();
        let sizes = tile_sizes(s.k_cut());
        assert!(sizes[0] >= 4.0 && sizes[1] < sizes[0]);
        for c in 0..CASCADES {
            let b = band(sizes, c, s.k_cut());
            let amps = amplitudes(&s, sea.dir, sizes[c], b, 7);
            // Each bin carries |h0(k)|² + |h0(−k)|²: the field's variance.
            let var: f32 = amps
                .iter()
                .map(|a| a[0] * a[0] + a[1] * a[1] + a[2] * a[2] + a[3] * a[3])
                .sum();
            // The band's variance by integral: ∫S dω over it.
            let (w0, w1) = (omega(b.0), omega(b.1));
            let n = 4000;
            let mut want = 0.0;
            for i in 0..n {
                let w = w0 + (w1 - w0) * (i as f32 + 0.5) / n as f32;
                want += s.at(w) * (w1 - w0) / n as f32;
            }
            // Random amplitudes: within 25% (thousands of bins; the coarse tile's lowest bins
            // carry most of it).
            assert!((var / want - 1.0).abs() < 0.25, "tile {c}: {var} vs {want}");
        }
    }

    #[test]
    fn the_field_is_real() {
        // conj(h0(−k)) of bin k is the conjugate of bin −k's h0: the evolved field is real.
        let sea = ocean(6.0);
        let s = Spectrum::of(&sea).unwrap();
        let sizes = tile_sizes(s.k_cut());
        let amps = amplitudes(&s, sea.dir, sizes[0], band(sizes, 0, s.k_cut()), 3);
        let half = N / 2;
        let at =
            |nx: i32, nz: i32| amps[(nz + half as i32) as usize * N + (nx + half as i32) as usize];
        for (nx, nz) in [(3, 1), (-5, 7), (10, -2)] {
            let a = at(nx, nz);
            let b = at(-nx, -nz);
            assert_eq!([a[2], a[3]], [b[0], -b[1]]);
        }
    }

    #[test]
    fn slopes_add_up_to_cox_and_munk() {
        // The drawn spectrum's slopes are of the same order as the glitter shows; the residual
        // makes up the rest.
        for wind in [3.0, 7.0, 12.0] {
            let s = Spectrum::of(&ocean(wind)).unwrap();
            let k_max = std::f32::consts::TAU / SHORTEST_M;
            let all = slope_variance(&s, 0.72 * s.wp * 0.72 * s.wp / G, k_max);
            let cm = cox_munk(wind);
            assert!(
                all > cm * 0.3 && all < cm * 2.5,
                "U {wind}: spectrum {all}, Cox–Munk {cm}"
            );
            assert!(residual_slope_variance(wind, all) <= cm);
        }
        assert_eq!(residual_slope_variance(0.0, 0.0), 0.0);
    }

    #[test]
    fn calm_water_has_no_detail() {
        assert!(Spectrum::of(&ocean(0.0)).is_none());
        let mut flat = ocean(8.0);
        flat.height_scale = 0.0;
        assert!(Spectrum::of(&flat).is_none());
    }
}
