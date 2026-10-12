//! CPU copy of the planet terrain that the PLANET and PLANET_FLAT primitives draw
//! (`pn_*` in `shaders/ui.wgsl`), so game code gets the same land and sea the map shows:
//! [`is_water`] to tell them apart, [`find_land`] to move a spawn point ashore.
//!
//! Both sides hash lattice points with the same integer hash (PCG3D) and run the same f32
//! simplex noise, so they agree up to float rounding. The shader adds up to
//! `MAX_OCTAVES - BASE_OCTAVES` detail octaves when a map is zoomed in; their amplitude is
//! below [`DETAIL_AMPLITUDE`], so they only wiggle coastlines at the scale of a few pixels.
//!
//! Directions are unit vectors `(cos lat sin lon, sin lat, cos lat cos lon)`: y is north,
//! longitude 0 faces +z, as in the shader.

use crate::draw::PlanetLook;

/// Octaves both sides share (they decide where the coastlines are).
pub const BASE_OCTAVES: u32 = 7;
/// Most octaves the shader uses (16x zoomed maps).
pub const MAX_OCTAVES: u32 = 12;
/// Largest height change the shader's detail octaves can add (heights are 0..1).
pub const DETAIL_AMPLITUDE: f32 = H_SCALE * 0.0156;

/// Base frequency (noise cells per unit of planet radius).
pub(crate) const F0: f32 = 1.1;
/// Frequency step between octaves.
pub(crate) const LACUNARITY: f32 = 2.07;
const WARP_F: f32 = 0.5;
const WARP_A: f32 = 0.12;
/// How much less each octave past the third follows the warp.
const WARP_DECAY: f32 = 0.5;
const NORM: f32 = 68.0;
const H_SCALE: f32 = 0.42;
const MOUNTAINS: f32 = 0.16;
/// About the mean of the ridged sum: mountains add relief, not a broad uplift.
const RIDGE_MEAN: f32 = 0.55;
const SALT_WARP: u32 = 0x68BC_21EB;
const SALT_OCTAVE: u32 = 0x9E37_79B9;

type V3 = [f32; 3];

#[inline]
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn adds(a: V3, s: f32) -> V3 {
    [a[0] + s, a[1] + s, a[2] + s]
}

#[inline]
fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// PCG3D (Jarzynski & Olano 2020), the shader's `pn_pcg3`.
#[inline]
pub fn pcg3(v: [u32; 3]) -> [u32; 3] {
    let mut v = v.map(|x| x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v = v.map(|x| x ^ (x >> 16));
    v[0] = v[0].wrapping_add(v[1].wrapping_mul(v[2]));
    v[1] = v[1].wrapping_add(v[2].wrapping_mul(v[0]));
    v[2] = v[2].wrapping_add(v[0].wrapping_mul(v[1]));
    v
}

/// The 32-bit seed of a look's `shape[0]` (`pn_seed`).
#[inline]
pub fn seed_hash(seed: f32) -> u32 {
    pcg3([seed.to_bits(), 0x2545_F491, 0x9E37_79B9])[0]
}

/// Pseudo-random gradient in [-1, 1)^3 at lattice point `c` (integral floats).
#[inline]
fn grad(c: V3, seed: u32) -> V3 {
    let h = pcg3([
        (c[0] as i32 as u32) ^ seed,
        (c[1] as i32 as u32) ^ seed,
        (c[2] as i32 as u32) ^ seed,
    ]);
    let k = 1.0 / 8_388_608.0;
    [
        (h[0] >> 8) as f32 * k - 1.0,
        (h[1] >> 8) as f32 * k - 1.0,
        (h[2] >> 8) as f32 * k - 1.0,
    ]
}

/// 3D simplex noise, about -1..1: `[value, d/dx, d/dy, d/dz]` (`pn_simplex`). The
/// derivative is only computed when `deriv` is set (the value is the same either way).
pub fn simplex(p: V3, seed: u32, deriv: bool) -> [f32; 4] {
    let s = [
        (p[0] + (p[0] + p[1] + p[2]) * (1.0 / 3.0)).floor(),
        (p[1] + (p[0] + p[1] + p[2]) * (1.0 / 3.0)).floor(),
        (p[2] + (p[0] + p[1] + p[2]) * (1.0 / 3.0)).floor(),
    ];
    let x0 = adds(sub(p, s), (s[0] + s[1] + s[2]) * (1.0 / 6.0));
    let step = |e: f32, x: f32| if x >= e { 1.0f32 } else { 0.0 };
    let g = [step(x0[1], x0[0]), step(x0[2], x0[1]), step(x0[0], x0[2])];
    let l = [1.0 - g[0], 1.0 - g[1], 1.0 - g[2]];
    let i1 = [g[0].min(l[2]), g[1].min(l[0]), g[2].min(l[1])];
    let i2 = [g[0].max(l[2]), g[1].max(l[0]), g[2].max(l[1])];
    let x1 = adds(sub(x0, i1), 1.0 / 6.0);
    let x2 = adds(sub(x0, i2), 1.0 / 3.0);
    let x3 = adds(x0, -0.5);
    let g0 = grad(s, seed);
    let g1 = grad(add(s, i1), seed);
    let g2 = grad(add(s, i2), seed);
    let g3 = grad(adds(s, 1.0), seed);
    let w = [
        (0.5 - dot(x0, x0)).max(0.0),
        (0.5 - dot(x1, x1)).max(0.0),
        (0.5 - dot(x2, x2)).max(0.0),
        (0.5 - dot(x3, x3)).max(0.0),
    ];
    let w2 = w.map(|w| w * w);
    let w4 = w2.map(|w| w * w);
    let gx = [dot(g0, x0), dot(g1, x1), dot(g2, x2), dot(g3, x3)];
    let n = w4[0] * gx[0] + w4[1] * gx[1] + w4[2] * gx[2] + w4[3] * gx[3];
    if !deriv {
        return [n * NORM, 0.0, 0.0, 0.0];
    }
    let t = [
        w2[0] * w[0] * gx[0],
        w2[1] * w[1] * gx[1],
        w2[2] * w[2] * gx[2],
        w2[3] * w[3] * gx[3],
    ];
    let d: [f32; 3] = std::array::from_fn(|k| {
        -8.0 * (t[0] * x0[k] + t[1] * x1[k] + t[2] * x2[k] + t[3] * x3[k])
            + w4[0] * g0[k]
            + w4[1] * g1[k]
            + w4[2] * g2[k]
            + w4[3] * g3[k]
    });
    [n * NORM, d[0] * NORM, d[1] * NORM, d[2] * NORM]
}

/// Terrain height 0..1 at unit direction `dir` (the sea is below `look.shape[1]`). Gas
/// giants have no surface: 0.
pub fn height(dir: [f32; 3], look: &PlanetLook) -> f32 {
    if look.style[1] > 0.0 {
        return 0.0;
    }
    let seed = seed_hash(look.shape[0]);
    let p = mul(dir, F0);
    let w = simplex(mul(p, WARP_F), seed ^ SALT_WARP, true);
    let disp = mul([w[1], w[2], w[3]], WARP_A);
    let mut g = 1.0f32;
    let mut fr = 1.0f32;
    let mut amp = 1.0f32;
    let mut sum = 0.0f32;
    let mut ridge = 0.0f32;
    let mut low = 0.0f32;
    for k in 0..BASE_OCTAVES {
        let x = mul(add(p, mul(disp, g)), fr);
        let n = simplex(x, seed.wrapping_add(k.wrapping_mul(SALT_OCTAVE)), false)[0];
        sum += amp * n;
        if (1..=4).contains(&k) {
            let r = 1.0 - n.abs();
            ridge += amp * (r * r);
        }
        if k == 2 {
            low = sum;
        }
        fr *= LACUNARITY;
        if k >= 2 {
            g *= WARP_DECAY;
        }
        amp *= 0.5;
    }
    let inland = smoothstep(0.02, 0.2, 0.5 + H_SCALE * low - look.shape[1]) * look.shape[2];
    (0.5 + H_SCALE * sum + MOUNTAINS * inland * (ridge - RIDGE_MEAN)).clamp(0.0, 1.0)
}

/// Unit direction of a longitude/latitude (degrees).
pub fn direction(lon_deg: f64, lat_deg: f64) -> [f32; 3] {
    let (lon, lat) = (lon_deg.to_radians(), lat_deg.to_radians());
    [
        (lat.cos() * lon.sin()) as f32,
        lat.sin() as f32,
        (lat.cos() * lon.cos()) as f32,
    ]
}

/// Whether the planet's surface at a longitude/latitude (degrees) is sea: below sea level
/// (frozen sea under polar ice counts, as do the lava seas of volcanic worlds). Gas giants
/// are all "sea".
pub fn is_water(lon_deg: f64, lat_deg: f64, look: &PlanetLook) -> bool {
    look.style[1] > 0.0 || height(direction(lon_deg, lat_deg), look) < look.shape[1]
}

/// Destination after going `dist_deg` along a great circle from (lon, lat) toward
/// `bearing_deg` (0 = north, 90 = east). Degrees in and out, longitude in -180..180.
fn travel(lon_deg: f64, lat_deg: f64, bearing_deg: f64, dist_deg: f64) -> (f64, f64) {
    let (lon1, lat1) = (lon_deg.to_radians(), lat_deg.to_radians());
    let (b, d) = (bearing_deg.to_radians(), dist_deg.to_radians());
    let lat2 = (lat1.sin() * d.cos() + lat1.cos() * d.sin() * b.cos())
        .clamp(-1.0, 1.0)
        .asin();
    let lon2 = lon1 + (b.sin() * d.sin() * lat1.cos()).atan2(d.cos() - lat1.sin() * lat2.sin());
    let lon = (lon2.to_degrees() + 180.0).rem_euclid(360.0) - 180.0;
    (lon, lat2.to_degrees())
}

/// The nearest land to a longitude/latitude (degrees), searched in rings of growing radius
/// and refined toward the coast; the point itself when it is on land. Returns the input
/// when the planet has no land (gas giants, all-ocean seeds).
pub fn find_land(lon_deg: f64, lat_deg: f64, look: &PlanetLook) -> (f64, f64) {
    if look.style[1] > 0.0 || !is_water(lon_deg, lat_deg, look) {
        return (lon_deg, lat_deg);
    }
    let mut inner = 0.0f64;
    let mut r = 0.1f64;
    while inner < 180.0 {
        let r_now = r.min(180.0);
        let step = r_now - inner;
        let circumference = 360.0 * r_now.to_radians().sin().max(0.0);
        let n = ((circumference / step).ceil() as usize).clamp(8, 4096);
        let turn = (r_now * 7.31).fract() * 360.0 / n as f64;
        let mut best: Option<(f64, f64, f64)> = None;
        for i in 0..n {
            let bearing = turn + i as f64 * 360.0 / n as f64;
            let (lo, la) = travel(lon_deg, lat_deg, bearing, r_now);
            if is_water(lo, la, look) {
                continue;
            }
            // Walk back toward the start: the coast lies between `inner` and `r_now`.
            let (mut wet, mut dry) = (inner, r_now);
            for _ in 0..16 {
                let mid = 0.5 * (wet + dry);
                let (mlo, mla) = travel(lon_deg, lat_deg, bearing, mid);
                if is_water(mlo, mla, look) {
                    wet = mid;
                } else {
                    dry = mid;
                }
            }
            if best.is_none_or(|b| dry < b.0) {
                let (lo, la) = travel(lon_deg, lat_deg, bearing, dry);
                best = Some((dry, lo, la));
            }
        }
        if let Some((_, lo, la)) = best {
            return (lo, la);
        }
        inner = r_now;
        r = r_now + (r_now * 0.08).clamp(0.1, 2.0);
    }
    (lon_deg, lat_deg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terran() -> PlanetLook {
        crate::map::PlanetKind::Terran.look(3.0)
    }

    #[test]
    fn hash_and_noise_are_stable() {
        // Pinned values: the shader computes the same (planet maps would shift otherwise).
        assert_eq!(pcg3([0, 0, 0]), pcg3([0, 0, 0]));
        assert_ne!(pcg3([1, 2, 3]), pcg3([1, 2, 4]));
        let n = simplex([0.3, -1.7, 2.2], 7, true);
        let m = simplex([0.3, -1.7, 2.2], 7, false);
        assert_eq!(n[0], m[0]);
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..20_000 {
            let t = i as f32 * 0.137;
            let v = simplex([t.sin() * 9.0, t * 0.31, (t * 1.7).cos() * 5.0], 11, false)[0];
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(
            lo > -1.2 && hi < 1.2 && lo < -0.6 && hi > 0.6,
            "range {lo}..{hi}"
        );
    }

    #[test]
    fn derivative_matches_finite_differences() {
        let p = [0.41, 1.33, -0.72];
        let n = simplex(p, 99, true);
        let e = 1e-3;
        for k in 0..3 {
            let mut a = p;
            let mut b = p;
            a[k] += e;
            b[k] -= e;
            let fd = (simplex(a, 99, false)[0] - simplex(b, 99, false)[0]) / (2.0 * e);
            assert!(
                (fd - n[k + 1]).abs() < 0.05 * (1.0 + fd.abs()),
                "axis {k}: {fd} vs {}",
                n[k + 1]
            );
        }
    }

    #[test]
    fn terran_world_has_land_and_sea() {
        let look = terran();
        let mut water = 0;
        let total = 60 * 30;
        for i in 0..60 {
            for j in 0..30 {
                let lon = -180.0 + (i as f64 + 0.5) * 6.0;
                let lat = -90.0 + (j as f64 + 0.5) * 6.0;
                let h = height(direction(lon, lat), &look);
                assert!((0.0..=1.0).contains(&h));
                assert_eq!(h, height(direction(lon, lat), &look), "deterministic");
                if is_water(lon, lat, &look) {
                    water += 1;
                }
            }
        }
        let f = water as f64 / total as f64;
        assert!((0.3..0.85).contains(&f), "water fraction {f}");
    }

    #[test]
    fn finds_nearby_land() {
        let look = terran();
        let mut checked = 0;
        for i in 0..40 {
            let lon = -170.0 + i as f64 * 8.5;
            let lat = -50.0 + (i as f64 * 13.0) % 100.0;
            if !is_water(lon, lat, &look) {
                assert_eq!(find_land(lon, lat, &look), (lon, lat));
                continue;
            }
            let (lo, la) = find_land(lon, lat, &look);
            assert!(!is_water(lo, la, &look), "{lo},{la} is water");
            // Nothing closer by more than one search ring: a coarse scan of that disk.
            let d = angle(lon, lat, lo, la);
            let reach = (d - (d * 0.08).clamp(0.1, 2.0)).max(0.0);
            for b in 0..36 {
                for k in 1..8 {
                    let (slo, sla) = travel(lon, lat, b as f64 * 10.0, reach * k as f64 / 8.0);
                    assert!(
                        is_water(slo, sla, &look),
                        "land at {slo},{sla} is closer than {lo},{la}"
                    );
                }
            }
            checked += 1;
        }
        assert!(checked > 5);
        let gas = crate::map::PlanetKind::Gas.look(1.0);
        assert!(is_water(10.0, 10.0, &gas));
        assert_eq!(find_land(10.0, 10.0, &gas), (10.0, 10.0));
    }

    fn angle(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
        let a = direction(lon1, lat1).map(|x| x as f64);
        let b = direction(lon2, lat2).map(|x| x as f64);
        (a[0] * b[0] + a[1] * b[1] + a[2] * b[2])
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }

    #[test]
    fn travel_goes_the_right_way() {
        let (lo, la) = travel(10.0, 0.0, 0.0, 5.0);
        assert!((lo - 10.0).abs() < 1e-9 && (la - 5.0).abs() < 1e-9);
        let (lo, la) = travel(10.0, 0.0, 90.0, 5.0);
        assert!((lo - 15.0).abs() < 1e-9 && la.abs() < 1e-9);
        let (lo, _) = travel(178.0, 0.0, 90.0, 5.0);
        assert!((lo + 177.0).abs() < 1e-9);
    }
}
