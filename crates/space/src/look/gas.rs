//! Gas giants and cloud decks: Jupiter, Saturn, Uranus, Neptune, Venus and the generic banded
//! look.
//!
//! The giants are painted through their winds ([`super::flow`]): each texel looks up where its
//! cloud was a day or two ago and takes the band color and the cloud texture from there, so
//! the jets shear everything into streaks, eddies tear the band edges into filaments and the
//! vortices wind their insides into spirals. Named features (the Great Red Spot, white ovals,
//! festoons, Neptune's dark spot) sit in the flow as vortices and are colored where they are
//! now. Latitudes and longitudes are radians unless a name says `_deg`.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use engine_core::math::{Rng, Vec3};

use super::bake::{Paint, Painter, Texel};
use super::color::{Rgb, luma, mix, ramp, saturate_by, scale, srgb};
use super::flow::{Flow, Swirl, eddies, jet_profile, mps};
use super::noise::{Noise, dir, smooth, wrap_pi};

const DEG: f32 = PI / 180.0;

/// Size of the flow grid for a map `width` texels wide.
fn flow_width(width: u32) -> usize {
    (width as usize / 4).clamp(64, 512)
}

/// A colored oval riding a vortex.
#[derive(Debug, Clone, Copy)]
struct Spot {
    swirl: Swirl,
    color: Rgb,
    /// Darker (negative) or brighter (positive) rim.
    rim: f32,
    opacity: f32,
}

/// Spots bucketed by latitude, so a texel only looks at the few near it.
struct Spots {
    list: Vec<Spot>,
    /// Indices of the spots reaching each 1° latitude band (−90..90).
    buckets: Vec<Vec<u16>>,
}

impl Spots {
    fn new(list: Vec<Spot>) -> Self {
        let mut buckets = vec![Vec::new(); 180];
        for (i, s) in list.iter().enumerate() {
            // The stream function reaches ~4.6 radii.
            let reach = s.swirl.radius * 4.8 / DEG;
            let lat = s.swirl.lat / DEG;
            let lo = ((lat - reach + 90.0).floor().max(0.0)) as usize;
            let hi = ((lat + reach + 90.0).floor().max(0.0) as usize).min(179);
            for b in &mut buckets[lo..=hi] {
                b.push(i as u16);
            }
        }
        Self { list, buckets }
    }

    #[inline]
    fn near(&self, lat: f32) -> impl Iterator<Item = &Spot> {
        let b = ((lat / DEG + 90.0).floor().max(0.0) as usize).min(179);
        self.buckets[b].iter().map(|&i| &self.list[i as usize])
    }

    fn psi(&self, lat: f32, lon: f32) -> f32 {
        self.near(lat).map(|s| s.swirl.psi(lat, lon)).sum()
    }

    /// Paint the spots over `c` at (lat, lon); `tex` (about −1..1) textures their insides.
    #[inline]
    fn paint(&self, mut c: Rgb, lat: f32, lon: f32, tex: f32) -> Rgb {
        for s in self.near(lat) {
            if (lat - s.swirl.lat).abs() > s.swirl.radius * 1.5 {
                continue;
            }
            let (rho, ..) = s.swirl.rho(lat, lon);
            if rho < 1.5 {
                let rho = rho * (1.0 + 0.12 * tex);
                let body = scale(s.color, 1.0 + 0.12 * tex);
                let rim = smooth(0.7, 1.0, rho) * smooth(1.35, 1.0, rho);
                let spot = mix(body, scale(c, 1.0 + s.rim), rim);
                c = mix(c, spot, smooth(1.3, 0.8, rho) * s.opacity);
            }
        }
        c
    }
}

/// A list of zonal winds, (latitude°, m/s) south to north, as a jet profile.
fn jets(stops: &'static [(f32, f32)], radius_m: f32) -> impl Fn(f32) -> f32 + Sync {
    move |lat| jet_profile(stops, lat, radius_m)
}

// ------------------------------------------------------------------------------------ Jupiter

/// Jupiter: cream zones and red-brown belts with torn, filamentary edges, blue-gray festoons
/// along the North Equatorial Belt, white ovals, the Great Red Spot spinning in its hollow
/// with the turbulent wake of the South Equatorial Belt behind it, and mottled gray-brown
/// polar regions full of small storms.
pub(crate) struct Jupiter {
    n: Noise,
    n2: Noise,
    n3: Noise,
    flow: Flow,
    profile: Vec<(f32, Rgb)>,
    grs: Swirl,
    spots: Spots,
    /// Hot spots on the NEB's southern edge: longitude, festoon width (radians), length (°).
    festoons: Vec<(f32, f32, f32)>,
}

/// Longitude of the Great Red Spot (degrees east).
pub const GRS_LON_DEG: f32 = -115.0;

const JUPITER_M: f32 = 69_911_000.0;

/// Jupiter's zonal winds (latitude°, m/s): alternating jets at the belt edges, the fastest at
/// 24° N, retrograde along the north edge of the Great Red Spot.
const JUPITER_JETS: [(f32, f32); 29] = [
    (-90.0, 0.0),
    (-70.0, 5.0),
    (-63.0, 30.0),
    (-58.0, -10.0),
    (-53.0, 35.0),
    (-49.0, -10.0),
    (-45.1, 30.0),
    (-40.9, -15.0),
    (-35.9, 35.0),
    (-31.9, -30.0),
    (-25.9, 45.0),
    (-17.9, -60.0),
    (-12.0, 0.0),
    (-6.9, 140.0),
    (0.0, 90.0),
    (7.1, 140.0),
    (12.0, 20.0),
    (18.5, -30.0),
    (24.7, 160.0),
    (31.5, -30.0),
    (35.1, 35.0),
    (38.1, -15.0),
    (43.8, 30.0),
    (48.0, -10.0),
    (53.0, 30.0),
    (58.0, -10.0),
    (63.0, 25.0),
    (70.0, 5.0),
    (90.0, 0.0),
];

impl Jupiter {
    pub(crate) fn new(seed: u64, width: u32) -> Self {
        let n = Noise::new(seed);
        let n2 = Noise::new(seed ^ 0x51);
        let n3 = Noise::new(seed ^ 0x77);
        let v = |m: f32| mps(m, JUPITER_M);
        let grs = Swirl {
            lat: -22.4 * DEG,
            lon: GRS_LON_DEG * DEG,
            radius: 4.6 * DEG,
            stretch: 1.45,
            speed: v(120.0),
        };
        // Profile north to south (ascending latitude for `ramp`).
        let mut profile: Vec<(f32, Rgb)> = vec![
            (90.0, srgb!(0x8c8a87)),
            (66.0, srgb!(0x99958c)),
            (55.0, srgb!(0xa7a093)),
            (49.0, srgb!(0xb3a998)),
            (47.4, srgb!(0xa99b85)),
            (45.6, srgb!(0xb1a38d)),
            (43.8, srgb!(0xd2cabb)),
            (40.0, srgb!(0xdad2c3)),
            (38.4, srgb!(0xd0c7b7)),
            (37.8, srgb!(0xa39076)),
            (35.4, srgb!(0xa8947a)),
            (34.8, srgb!(0xdcd3c2)),
            (31.8, srgb!(0xe3dbca)),
            (31.2, srgb!(0x9d7a5f)),
            (28.0, srgb!(0xab8a6f)),
            (25.0, srgb!(0x9a7359)),
            (24.4, srgb!(0xeae2d1)),
            (21.0, srgb!(0xf1eadb)),
            (18.8, srgb!(0xe8dfcc)),
            (18.2, srgb!(0x8a6048)),
            (15.5, srgb!(0x855b43)),
            (12.0, srgb!(0x986d53)),
            (8.6, srgb!(0x835a43)),
            (7.4, srgb!(0x91684f)),
            (6.8, srgb!(0xdcc6a5)),
            (3.0, srgb!(0xe6d2b1)),
            (0.8, srgb!(0xdcc39e)),
            (0.0, srgb!(0xd5bb95)),
            (-0.8, srgb!(0xdcc39e)),
            (-4.0, srgb!(0xe8d8bb)),
            (-6.6, srgb!(0xe3d1b1)),
            (-7.2, srgb!(0x9a6f53)),
            (-9.4, srgb!(0xa07657)),
            (-11.2, srgb!(0xb68e6e)),
            (-14.0, srgb!(0xbb9574)),
            (-16.0, srgb!(0xa78063)),
            (-17.6, srgb!(0x9c7256)),
            (-18.2, srgb!(0xebe3d2)),
            (-22.0, srgb!(0xf1eadb)),
            (-25.6, srgb!(0xe9e1d0)),
            (-26.2, srgb!(0xa78b73)),
            (-29.0, srgb!(0xac9178)),
            (-31.6, srgb!(0xa0846b)),
            (-32.2, srgb!(0xe0d8c9)),
            (-35.6, srgb!(0xe3dbcc)),
            (-36.2, srgb!(0xb5a48d)),
            (-40.6, srgb!(0xb09f89)),
            (-41.2, srgb!(0xd5cec0)),
            (-44.8, srgb!(0xd1c9bb)),
            (-45.4, srgb!(0xb6ac9c)),
            (-50.0, srgb!(0xafa698)),
            (-56.0, srgb!(0xa4a094)),
            (-67.0, srgb!(0x97958d)),
            (-90.0, srgb!(0x8d8c8a)),
        ];
        profile.reverse();

        let mut rng = Rng::new(seed ^ 0x0a1);
        let mut spots = Vec::new();
        let white = srgb!(0xf5f1e8);
        // Oval BA (reddish now) in the South Temperate Belt.
        spots.push(Spot {
            swirl: Swirl {
                lat: -33.2 * DEG,
                lon: (GRS_LON_DEG - 58.0) * DEG,
                radius: 1.9 * DEG,
                stretch: 1.35,
                speed: v(100.0),
            },
            color: srgb!(0xe0bfa2),
            rim: -0.25,
            opacity: 0.95,
        });
        // The string of pearls (southern anticyclones turn counterclockwise).
        for k in 0..8 {
            spots.push(Spot {
                swirl: Swirl {
                    lat: (-40.3 + rng.range(-0.5, 0.5)) * DEG,
                    lon: (GRS_LON_DEG + 40.0 + k as f32 * 11.0 + rng.range(-2.0, 2.0)) * DEG,
                    radius: rng.range(0.7, 1.0) * DEG,
                    stretch: 1.3,
                    speed: v(60.0),
                },
                color: white,
                rim: -0.3,
                opacity: 0.9,
            });
        }
        // Small white ovals in the temperate latitudes.
        for _ in 0..16 {
            let north = rng.chance(0.5);
            let lat = if north {
                rng.range(28.0, 47.0)
            } else {
                rng.range(-50.0, -27.0)
            };
            spots.push(Spot {
                swirl: Swirl {
                    lat: lat * DEG,
                    lon: rng.range(-180.0, 180.0) * DEG,
                    radius: rng.range(0.4, 0.8) * DEG,
                    stretch: 1.3,
                    speed: v(40.0) * if north { -1.0 } else { 1.0 },
                },
                color: white,
                rim: -0.2,
                opacity: rng.range(0.5, 0.85),
            });
        }
        // Brown barges along the North Equatorial Belt (cyclones).
        for k in 0..4 {
            spots.push(Spot {
                swirl: Swirl {
                    lat: (14.5 + rng.range(-1.0, 1.0)) * DEG,
                    lon: (-150.0 + k as f32 * 85.0 + rng.range(-15.0, 15.0)) * DEG,
                    radius: 0.75 * DEG,
                    stretch: 2.6,
                    speed: v(40.0),
                },
                color: srgb!(0x654031),
                rim: 0.0,
                opacity: 0.85,
            });
        }
        // The polar regions are crowded with small storms, mostly cyclones.
        for _ in 0..140 {
            let north = rng.chance(0.5);
            let lat = rng.range(52.0, 80.0) * if north { 1.0 } else { -1.0 };
            let cyclone = rng.chance(0.7);
            let ccw = cyclone == north;
            let bright = !cyclone && rng.chance(0.6);
            spots.push(Spot {
                swirl: Swirl {
                    lat: lat * DEG,
                    lon: rng.range(-180.0, 180.0) * DEG,
                    radius: rng.range(0.5, 1.6) * DEG,
                    stretch: 1.15,
                    speed: v(rng.range(25.0, 60.0)) * if ccw { 1.0 } else { -1.0 },
                },
                color: if bright {
                    srgb!(0xd9d5cb)
                } else {
                    srgb!(0x8a8479)
                },
                rim: if bright { -0.15 } else { 0.06 },
                opacity: rng.range(0.12, 0.4),
            });
        }
        let spots = Spots::new(spots);
        let festoons = (0..10)
            .map(|k| {
                (
                    (-180.0 + k as f32 * 36.0 + rng.range(-12.0, 12.0)) * DEG,
                    rng.range(0.5, 1.1) * DEG,
                    rng.range(3.0, 6.0),
                )
            })
            .collect();

        let wn = Noise::new(seed ^ 0x3a4);
        let flow = Flow::new(
            flow_width(width),
            jets(&JUPITER_JETS, JUPITER_M),
            |d, lat, lon| {
                let e = eddies(&wn, d, 10.0, 5, v(Self::turbulence(lat, lon)));
                e + grs.psi(lat, lon) + spots.psi(lat, lon)
            },
            6.0,
            36,
        );
        Self {
            n,
            n2,
            n3,
            flow,
            profile,
            grs,
            spots,
            festoons,
        }
    }

    /// Typical eddy wind (m/s) at a latitude/longitude: calm zones, churning belts, the
    /// chaos behind the Great Red Spot and the stormy polar regions.
    fn turbulence(lat: f32, lon: f32) -> f32 {
        let lat_deg = lat / DEG;
        let belt = |lo: f32, hi: f32| {
            smooth(lo - 1.5, lo + 1.5, lat_deg) * smooth(hi + 1.5, hi - 1.5, lat_deg)
        };
        let mut amp = 5.0;
        amp += 8.0 * belt(7.0, 18.5) + 6.0 * belt(-17.9, -7.0) + 5.0 * belt(24.7, 31.5);
        amp += 4.0 * belt(-31.9, -25.9) + 3.0 * belt(-6.5, 6.5);
        // The wake west of the Great Red Spot.
        let west = wrap_pi(GRS_LON_DEG * DEG - lon);
        amp += 22.0 * smooth(0.05, 0.2, west) * smooth(1.6, 0.6, west) * belt(-17.5, -8.0);
        amp + 12.0 * smooth(40.0, 60.0, lat_deg.abs())
    }

    /// The Great Red Spot and its hollow over `bg`; `tex` is texture from the cloud's source.
    #[inline]
    fn grs_color(&self, rho: f32, tex: f32, fine: f32, bg: Rgb) -> Rgb {
        if rho > 1.75 {
            return bg;
        }
        // The hollow around it: bright zone, swept clean.
        let hollow = srgb!(0xece4d4);
        let mut c = mix(bg, hollow, smooth(1.6, 1.25, rho) * 0.5);
        // The red oval fills most of the vortex; a ragged, spiral-textured edge.
        let rho = rho / 1.2 * (1.0 + 0.07 * tex);
        if rho < 1.12 {
            let core = srgb!(0xcf9a74);
            let body = srgb!(0xbf6e4c);
            let outer = srgb!(0xad5d42);
            let mut spot = ramp(
                &[(0.0, core), (0.3, core), (0.65, body), (0.95, outer)],
                rho,
            );
            spot = scale(spot, 1.0 + 0.28 * tex + 0.06 * fine);
            spot = saturate_by(spot, 1.0 + 0.2 * tex);
            // A thin darker collar where the spot meets the hollow.
            let collar = smooth(0.9, 1.0, rho) * smooth(1.12, 1.02, rho);
            spot = mix(spot, srgb!(0x91573f), collar * 0.45);
            c = mix(c, spot, smooth(1.1, 0.96, rho));
        }
        c
    }
}

impl Painter for Jupiter {
    fn paint(&self, t: &Texel) -> Paint {
        let (slat, slon) = self.flow.source(t.lat, t.lon);
        let ds = dir(slat, slon);
        // Cloud texture carried along with the flow: sheared into threads and curls.
        let fine = self.n.fbm(ds * 34.0, 3, 0.55);
        let threads = self
            .n2
            .fbm(Vec3::new(ds.x * 12.0, ds.y * 60.0, ds.z * 12.0), 3, 0.5);
        let lw = (slat + threads * 0.08 * DEG) / DEG;
        let mut c = ramp(&self.profile, lw);
        // Ochre tints drifting along the Equatorial Zone, browner patches in the belts.
        let slow = self.n3.fbm(ds * 2.4, 2, 0.5);
        let ez = smooth(8.0, 3.0, lw.abs());
        c = mix(
            c,
            mix(c, srgb!(0xd7b07e), 0.6),
            ez * smooth(-0.35, 0.5, slow),
        );
        let belts = 1.0 - luma(c).min(0.7) / 0.7;
        c = saturate_by(c, 1.0 + 0.3 * belts * slow);
        c = scale(c, 1.0 + 0.08 * fine + 0.035 * threads);
        // Polar mottling.
        let polar = smooth(46.0, 62.0, lw.abs());
        if polar > 0.0 {
            let m = self.n3.fbm(ds * 11.0, 4, 0.55);
            c = mix(c, scale(c, 1.0 + 0.22 * m), polar);
        }

        // Festoons: blue-gray plumes hanging from the NEB's southern edge into the
        // Equatorial Zone (the flow drags them south-west), white plumes beside them.
        let slat_deg = slat / DEG;
        if (-1.0..9.0).contains(&slat_deg) {
            for &(f, width, length) in &self.festoons {
                let s = wrap_pi(slon - f);
                if s.abs() > 10.0 * DEG {
                    continue;
                }
                // The hot spot: a dark blue-gray bay in the belt's edge.
                let bay = (-(s / (2.2 * DEG)).powi(2) - ((slat_deg - 6.4) / 0.9).powi(2)).exp();
                // The festoon hangs from it, thinning as it goes.
                let along = (7.0 - slat_deg) / length;
                let reach = smooth(7.6, 6.6, slat_deg) * smooth(1.05, 0.6, along);
                let w = width * (1.0 - 0.5 * along.clamp(0.0, 1.0));
                let k = ((-(s / w).powi(2)).exp() * reach).max(bay * 0.85);
                let broken = 0.75 + 0.35 * self.n3.at(ds * 50.0);
                c = mix(c, srgb!(0x5b6574), (k * broken).clamp(0.0, 1.0) * 0.7);
                // A white plume to the east of each hot spot.
                let s2 = wrap_pi(slon - f - 5.5 * DEG);
                let plume = (-(s2 / (3.0 * DEG)).powi(2) - ((slat_deg - 4.2) / 1.9).powi(2)).exp();
                c = mix(c, srgb!(0xf5f0e6), plume * 0.55 * (0.8 + 0.4 * fine));
            }
        }
        // White convective rifts in the wake of the Great Red Spot.
        let west = wrap_pi(GRS_LON_DEG * DEG - slon);
        let wake = smooth(0.05, 0.2, west)
            * smooth(1.4, 0.5, west)
            * smooth(-18.0, -15.5, slat_deg)
            * smooth(-7.5, -10.0, slat_deg);
        if wake > 0.0 {
            let r = self.n3.fbm(ds * 24.0, 3, 0.55);
            c = mix(c, srgb!(0xf0eadc), smooth(0.1, 0.5, r) * wake * 0.85);
        }

        c = self.spots.paint(c, t.lat, t.lon, fine);
        let (rho, ..) = self.grs.rho(t.lat, t.lon);
        if rho < 1.75 {
            c = self.grs_color(rho, self.n3.fbm(ds * 16.0, 4, 0.6), fine, c);
        }
        Paint {
            color: c,
            ..Default::default()
        }
    }
}

// ------------------------------------------------------------------------------------- Saturn

/// Saturn: soft butterscotch bands of low contrast drawn out by very fast winds, a blue-gray
/// north polar region inside the hexagon, and a dark polar vortex at each pole.
pub(crate) struct Saturn {
    n: Noise,
    n2: Noise,
    flow: Flow,
    profile: Vec<(f32, Rgb)>,
    spots: Spots,
}

const SATURN_M: f32 = 58_232_000.0;

/// Saturn's winds: a broad, very fast equatorial jet and narrower jets toward the poles.
const SATURN_JETS: [(f32, f32); 26] = [
    (-90.0, 0.0),
    (-88.0, 140.0),
    (-84.0, 0.0),
    (-78.0, 60.0),
    (-72.0, -10.0),
    (-62.0, 90.0),
    (-55.0, -20.0),
    (-47.0, 90.0),
    (-41.0, -10.0),
    (-35.0, 40.0),
    (-25.0, 250.0),
    (-12.0, 380.0),
    (0.0, 430.0),
    (12.0, 380.0),
    (25.0, 250.0),
    (35.0, 40.0),
    (41.0, -10.0),
    (47.0, 90.0),
    (55.0, -20.0),
    (62.0, 90.0),
    (70.0, -10.0),
    (76.0, 110.0),
    (80.0, -20.0),
    (84.0, 0.0),
    (88.0, 140.0),
    (90.0, 0.0),
];

impl Saturn {
    pub(crate) fn new(seed: u64, width: u32) -> Self {
        let mut profile: Vec<(f32, Rgb)> = vec![
            (90.0, srgb!(0x5a6064)),
            (88.0, srgb!(0x7b8383)),
            (80.0, srgb!(0x91978e)),
            (77.5, srgb!(0x878370)),
            (75.5, srgb!(0x9d9e92)),
            (70.0, srgb!(0xa8a590)),
            (62.0, srgb!(0xbbaf8a)),
            (55.0, srgb!(0xc8b78c)),
            (48.0, srgb!(0xd1be8f)),
            (46.5, srgb!(0xc2ac7b)),
            (42.0, srgb!(0xccb583)),
            (38.0, srgb!(0xd9c594)),
            (33.0, srgb!(0xd1b985)),
            (28.0, srgb!(0xdbc696)),
            (24.0, srgb!(0xceb37b)),
            (19.0, srgb!(0xc7a86f)),
            (15.0, srgb!(0xd7be87)),
            (9.0, srgb!(0xe5d3a5)),
            (3.0, srgb!(0xeadbb2)),
            (0.0, srgb!(0xe7d6aa)),
            (-3.0, srgb!(0xeadbb2)),
            (-8.0, srgb!(0xe2ce9d)),
            (-14.0, srgb!(0xd5ba81)),
            (-19.0, srgb!(0xc7a86e)),
            (-24.0, srgb!(0xd2b883)),
            (-30.0, srgb!(0xdbc593)),
            (-35.0, srgb!(0xcfb682)),
            (-41.0, srgb!(0xd7c290)),
            (-47.0, srgb!(0xc8b182)),
            (-55.0, srgb!(0xc1af85)),
            (-65.0, srgb!(0xb2a581)),
            (-75.0, srgb!(0xa1997d)),
            (-85.0, srgb!(0x8b8575)),
            (-88.5, srgb!(0x6e695d)),
            (-90.0, srgb!(0x59554f)),
        ];
        profile.reverse();
        let v = |m: f32| mps(m, SATURN_M);
        let mut rng = Rng::new(seed ^ 0x5a7);
        let mut list = Vec::new();
        // A few small pale storms in the storm alleys.
        for _ in 0..10 {
            let north = rng.chance(0.5);
            let lat = rng.range(32.0, 46.0) * if north { 1.0 } else { -1.0 };
            list.push(Spot {
                swirl: Swirl {
                    lat: lat * DEG,
                    lon: rng.range(-180.0, 180.0) * DEG,
                    radius: rng.range(0.5, 1.1) * DEG,
                    stretch: 1.4,
                    speed: v(50.0) * if north { -1.0 } else { 1.0 },
                },
                color: srgb!(0xece0c2),
                rim: -0.06,
                opacity: rng.range(0.25, 0.5),
            });
        }
        let spots = Spots::new(list);
        let wn = Noise::new(seed ^ 0x99);
        let flow = Flow::new(
            flow_width(width),
            jets(&SATURN_JETS, SATURN_M),
            |d, lat, lon| {
                let a = lat.abs() / DEG;
                let speed = 5.0 + 7.0 * smooth(30.0, 45.0, a) + 8.0 * smooth(60.0, 75.0, a);
                eddies(&wn, d, 12.0, 4, v(speed)) + spots.psi(lat, lon)
            },
            1.4,
            20,
        );
        Self {
            n: Noise::new(seed),
            n2: Noise::new(seed ^ 0x2e),
            flow,
            profile,
            spots,
        }
    }
}

impl Painter for Saturn {
    fn paint(&self, t: &Texel) -> Paint {
        let (slat, slon) = self.flow.source(t.lat, t.lon);
        let ds = dir(slat, slon);
        let fine = self.n.fbm(ds * 30.0, 3, 0.55);
        let lw = (slat + fine * 0.25 * DEG) / DEG;
        let mut c = ramp(&self.profile, lw);
        // Hexagon around the north pole: a darker band where its jet runs, blue-gray inside.
        let colat = FRAC_PI_2 - t.lat;
        if colat < 22.0 * DEG {
            let a = (t.lon * 6.0 + 0.4).rem_euclid(TAU) / 6.0 - PI / 6.0;
            let wobble = 0.25 * DEG * self.n2.at(t.d * 20.0);
            let r_hex = 13.0 * DEG * (PI / 6.0).cos() / a.cos() + wobble;
            let band = (-((colat - r_hex) / (1.0 * DEG)).powi(2)).exp();
            let inside = smooth(r_hex + 0.8 * DEG, r_hex - 0.8 * DEG, colat);
            c = mix(c, srgb!(0x8d9a9a), inside * 0.5);
            c = mix(c, srgb!(0x75776a), band * 0.55);
            // The polar vortex's eye.
            c = mix(
                c,
                srgb!(0x4f5458),
                smooth(2.2 * DEG, 0.6 * DEG, colat) * 0.7,
            );
        }
        let slow = self.n2.fbm(ds * 2.0, 2, 0.5);
        c = saturate_by(c, 1.0 + 0.15 * slow);
        let threads = self.n2.fbm(ds * 90.0, 2, 0.5);
        c = scale(c, 1.0 + 0.035 * fine + 0.02 * threads + 0.025 * slow);
        c = self.spots.paint(c, t.lat, t.lon, fine);
        Paint {
            color: c,
            ..Default::default()
        }
    }
}

// ------------------------------------------------------------------------------------- Uranus

/// Uranus: pale cyan, nearly featureless; faint bands, a brighter polar cap and a few small
/// bright clouds.
pub(crate) struct Uranus {
    n: Noise,
    clouds: Vec<Swirl>,
}

impl Uranus {
    pub(crate) fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let clouds = (0..6)
            .map(|_| Swirl {
                lat: rng.range(20.0, 45.0) * DEG * if rng.chance(0.6) { 1.0 } else { -1.0 },
                lon: rng.range(-PI, PI),
                radius: rng.range(0.6, 1.2) * DEG,
                stretch: rng.range(2.5, 5.0),
                speed: 0.0,
            })
            .collect();
        Self {
            n: Noise::new(seed),
            clouds,
        }
    }
}

impl Painter for Uranus {
    fn paint(&self, t: &Texel) -> Paint {
        let lat_deg = t.lat / DEG;
        let w = self
            .n
            .fbm(Vec3::new(t.d.x, t.d.y * 5.0, t.d.z) * 3.0, 3, 0.5)
            * 1.5;
        let l = lat_deg + w;
        let mut c = ramp(
            &[
                (-90.0, srgb!(0xa9d3d8)),
                (-50.0, srgb!(0xa6d0d6)),
                (-40.0, srgb!(0x9dc8cf)),
                (-20.0, srgb!(0xa3cfd5)),
                (0.0, srgb!(0xa1ccd3)),
                (20.0, srgb!(0xa5d0d6)),
                (40.0, srgb!(0x9ac6ce)),
                (50.0, srgb!(0xa9d4d9)),
                (62.0, srgb!(0xc1e2e4)),
                (90.0, srgb!(0xcde8e9)),
            ],
            l,
        );
        c = scale(
            c,
            1.0 + 0.012 * (l * 0.9).sin() + 0.01 * self.n.at(t.d * 40.0),
        );
        for v in &self.clouds {
            if (t.lat - v.lat).abs() > v.radius * 2.0 {
                continue;
            }
            let (rho, ..) = v.rho(t.lat, t.lon);
            if rho < 2.0 {
                c = mix(c, srgb!(0xe9f6f6), smooth(1.4, 0.3, rho) * 0.6);
            }
        }
        Paint {
            color: c,
            ..Default::default()
        }
    }
}

// ------------------------------------------------------------------------------------ Neptune

/// Neptune: deep blue with darker and paler bands, the Great Dark Spot with its bright
/// companion clouds, a smaller dark spot with a bright core, and white streaks of high
/// methane cloud that the fastest winds in the solar system draw out along the latitudes.
pub(crate) struct Neptune {
    n: Noise,
    n2: Noise,
    flow: Flow,
    dark: Vec<(Swirl, f32)>,
}

const NEPTUNE_M: f32 = 24_622_000.0;

/// Retrograde at the equator, prograde near ±70°.
const NEPTUNE_JETS: [(f32, f32); 9] = [
    (-90.0, 0.0),
    (-72.0, 250.0),
    (-50.0, 0.0),
    (-25.0, -280.0),
    (0.0, -400.0),
    (25.0, -280.0),
    (50.0, 0.0),
    (72.0, 250.0),
    (90.0, 0.0),
];

impl Neptune {
    pub(crate) fn new(seed: u64, width: u32) -> Self {
        let v = |m: f32| mps(m, NEPTUNE_M);
        // Anticyclones in the south turn counterclockwise.
        let dark = vec![
            (
                Swirl {
                    lat: -20.0 * DEG,
                    lon: 15.0 * DEG,
                    radius: 4.2 * DEG,
                    stretch: 1.7,
                    speed: v(100.0),
                },
                0.85,
            ),
            (
                Swirl {
                    lat: -54.0 * DEG,
                    lon: 95.0 * DEG,
                    radius: 1.8 * DEG,
                    stretch: 1.4,
                    speed: v(80.0),
                },
                0.75,
            ),
        ];
        let wn = Noise::new(seed ^ 0x77);
        let flow = Flow::new(
            flow_width(width).min(512),
            jets(&NEPTUNE_JETS, NEPTUNE_M),
            |d, lat, lon| {
                let mut p = eddies(&wn, d, 10.0, 3, v(14.0));
                for (s, _) in &dark {
                    p += s.psi(lat, lon);
                }
                p
            },
            0.6,
            16,
        );
        Self {
            n: Noise::new(seed),
            n2: Noise::new(seed ^ 0x4e),
            flow,
            dark,
        }
    }
}

impl Painter for Neptune {
    fn paint(&self, t: &Texel) -> Paint {
        let (slat, slon) = self.flow.source(t.lat, t.lon);
        let ds = dir(slat, slon);
        let fine = self.n.fbm(ds * 20.0, 3, 0.55);
        let lw = (slat + fine * 1.2 * DEG) / DEG;
        let mut c = ramp(
            &[
                (-90.0, srgb!(0x5b80ca)),
                (-75.0, srgb!(0x5078c8)),
                (-64.0, srgb!(0x3d5eae)),
                (-55.0, srgb!(0x304c99)),
                (-47.0, srgb!(0x3f63b6)),
                (-30.0, srgb!(0x486ec4)),
                (-12.0, srgb!(0x3c62ba)),
                (0.0, srgb!(0x3a5eb8)),
                (15.0, srgb!(0x4068bf)),
                (30.0, srgb!(0x466ec5)),
                (50.0, srgb!(0x3f64b8)),
                (70.0, srgb!(0x4a6ec0)),
                (90.0, srgb!(0x5578c4)),
            ],
            lw,
        );
        c = scale(c, 1.0 + 0.06 * fine + 0.02 * self.n2.at(ds * 80.0));
        for (s, k) in &self.dark {
            let (rho, ..) = s.rho(t.lat, t.lon);
            if rho < 1.8 {
                let tex = 1.0 + 0.12 * self.n2.fbm(ds * 12.0, 3, 0.5);
                c = mix(c, scale(srgb!(0x1d2c6a), tex), smooth(1.2, 0.7, rho) * k);
            }
        }
        // The small dark spot's bright core.
        let (rho, ..) = self.dark[1].0.rho(t.lat, t.lon);
        c = mix(c, srgb!(0xdfe8f6), smooth(0.45, 0.1, rho) * 0.8);
        // High white clouds: patches in their latitudes, sheared into streaks by the flow;
        // bright companions along the Great Dark Spot's southern edge.
        let a = slat / DEG;
        let belt = |lo: f32, hi: f32| smooth(lo, lo + 4.0, a) * smooth(hi, hi - 4.0, a);
        let mut cover =
            0.55 * belt(20.0, 48.0) + 0.6 * belt(-42.0, -18.0) + 0.5 * belt(-74.0, -60.0);
        let (gds, _, dy) = self.dark[0].0.rho(t.lat, t.lon);
        cover += 0.9 * smooth(1.6, 1.15, gds) * smooth(0.9, 1.05, gds) * smooth(-0.3, -0.8, dy);
        let clouds = self.n2.fbm(ds * 9.0 + Vec3::splat(3.0), 4, 0.6);
        let white = smooth(0.95 - cover, 1.3 - cover, clouds + 0.25 * fine);
        c = mix(c, srgb!(0xf1f5fb), white * 0.92);
        Paint {
            color: c,
            ..Default::default()
        }
    }
}

// -------------------------------------------------------------------------------------- Venus

/// A thick, bright cloud deck (Venus): pale yellow in the body's color, with faint darker
/// streaks in the sideways "Y" the superrotating winds draw, brighter polar collars and
/// zonal banding.
pub(crate) struct Venus {
    base: Rgb,
    n: Noise,
    n2: Noise,
}

impl Venus {
    pub(crate) fn new(color: [f32; 3], seed: u64, _width: u32) -> Self {
        Self {
            base: color,
            n: Noise::new(seed),
            n2: Noise::new(seed ^ 0x1e),
        }
    }
}

impl Painter for Venus {
    fn paint(&self, t: &Texel) -> Paint {
        let d = t.d;
        let lat_deg = t.lat / DEG;
        let w = self.n.fbm(Vec3::new(d.x, d.y * 4.0, d.z) * 2.0, 4, 0.5) * 6.0;
        let l = lat_deg + w;
        // The Y: a stem along the equator that splits into two arms toward ±35°.
        let x = wrap_pi(t.lon + 0.6) / PI; // -1..1 around the planet
        let arm = 35.0 * smooth(-0.15, 0.6, x);
        let stem = (-((l.abs() - arm) / (6.0 + 6.0 * smooth(0.0, 0.6, x))).powi(2)).exp()
            * smooth(-0.9, -0.5, x)
            * smooth(0.95, 0.7, x);
        let streak = self.n2.fbm(Vec3::new(d.x, d.y * 7.0, d.z) * 6.0, 3, 0.55);
        let mut k = 1.0 - 0.07 * stem - 0.03 * streak;
        // Bright collars near ±65°, darker vortex at the poles.
        k += 0.035 * (-((l.abs() - 64.0) / 5.0).powi(2)).exp();
        k -= 0.05 * smooth(75.0, 88.0, l.abs());
        let tint = mix(self.base, mix(self.base, srgb!(0xd9c08c), 0.6), 0.6 * stem);
        let c = scale(tint, k + 0.012 * self.n.at(d * 60.0));
        Paint {
            color: c,
            ..Default::default()
        }
    }
}

// ----------------------------------------------------------------------------------- Banded

/// A generic gas giant: bands cycling through `colors`, alternating jets at their edges that
/// tear them into filaments, and an optional great storm (for bodies that aren't in the
/// solar system).
pub(crate) struct Banded {
    colors: Vec<Rgb>,
    bands: f32,
    storm: Option<(Swirl, Rgb)>,
    base: Rgb,
    n: Noise,
    flow: Flow,
}

impl Banded {
    pub(crate) fn new(
        colors: Vec<Rgb>,
        bands: f32,
        storm: Option<Rgb>,
        base: Rgb,
        seed: u64,
        width: u32,
    ) -> Self {
        let bands = bands.max(1.0);
        let storm = storm.map(|c| {
            (
                Swirl {
                    lat: -22.0 * DEG,
                    lon: 0.8,
                    radius: 4.0 * DEG,
                    stretch: 1.5,
                    speed: 0.12,
                },
                c,
            )
        });
        let wn = Noise::new(seed ^ 0xba);
        let flow = Flow::new(
            flow_width(width).min(512),
            // A jet at every band edge, alternating in direction.
            move |lat| 0.12 * ((lat.sin() * 0.5 + 0.5) * bands * PI).cos(),
            |d, lat, lon| {
                let s = storm.map(|(s, _)| s.psi(lat, lon)).unwrap_or(0.0);
                eddies(&wn, d, 12.0, 4, 0.02) + s
            },
            2.0,
            16,
        );
        Self {
            colors,
            bands,
            storm,
            base,
            n: Noise::new(seed),
            flow,
        }
    }
}

impl Painter for Banded {
    fn paint(&self, t: &Texel) -> Paint {
        if self.colors.is_empty() {
            return Paint {
                color: self.base,
                ..Default::default()
            };
        }
        let (slat, slon) = self.flow.source(t.lat, t.lon);
        let ds = dir(slat, slon);
        let fine = self.n.fbm(ds * 30.0, 3, 0.55);
        let x = (slat.sin() * 0.5 + 0.5) * self.bands + 0.05 * fine;
        let i = x.floor() as i64;
        let f = smooth(0.35, 0.65, x - x.floor());
        let n = self.colors.len() as i64;
        let a = self.colors[i.rem_euclid(n) as usize];
        let b = self.colors[(i + 1).rem_euclid(n) as usize];
        let mut c = scale(mix(a, b, f), 1.0 + 0.06 * fine);
        if let Some((s, color)) = &self.storm {
            let (rho, ..) = s.rho(t.lat, t.lon);
            c = mix(
                c,
                scale(c, 1.12),
                smooth(1.6, 1.0, rho) * smooth(0.9, 1.05, rho),
            );
            c = mix(
                c,
                scale(*color, 1.0 + 0.15 * fine),
                smooth(1.05, 0.8, rho) * 0.9,
            );
        }
        Paint {
            color: c,
            ..Default::default()
        }
    }
}
