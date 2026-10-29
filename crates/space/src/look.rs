//! How bodies look from afar.
//!
//! Every body that is not a star gets a far-view impostor (and Saturn its rings) as a child
//! entity, drawn by [`BodiesFeature`]: a ray-traced spheroid, exactly round and anti-aliased
//! at any size, textured with baked maps of the body (Jupiter's belts and Great Red Spot,
//! Earth's continents and clouds, the Moon's maria and ray craters, …), lit with a model that
//! suits it (Minnaert limb darkening for gas giants and cloud decks, Lunar-Lambert for dusty
//! airless bodies, a sun glint on Earth's oceans, relief at the terminator), with a glowing
//! rim of haze where a body has one, ring shadows on the planet and the planet's shadow on
//! its rings. Far away it is kept at least a few pixels wide, so planets shine as dots in the
//! night sky the way they do in reality.
//!
//! The impostor writes the depth of a sphere a little inside the body's radius
//! ([`IMPOSTOR_SCALE`]), so a terrain on the same body always covers it up close.
//!
//! Maps are baked on the CPU the first time a body shows up: a small 256×128 map at once,
//! and the full 2048×1024 one in the background when the body grows past a few dozen pixels
//! ([`LookBakes`] reports progress and timings).

mod bake;
mod color;
mod earth;
mod flow;
mod gas;
pub mod noise;
mod render;
mod rock;
mod world;

use engine_core::math::Vec3;
use engine_core::prelude::*;
use engine_render::prelude::*;

use crate::{BodyKind, CelestialBody, SpaceSettings};

pub use bake::{
    SLOPE_SCALE, SurfaceMaps, bake_rings, bake_surface, bake_surface_on, surface_color,
};
/// Longitude (degrees east) where the far look puts Jupiter's Great Red Spot.
pub use gas::GRS_LON_DEG;
pub use render::{BodiesFeature, SHADER as BODIES_SHADER};

/// The depth the far view writes is that of a sphere at this fraction of the body's radius
/// (below any terrain's lowest point: 32 km under Earth's sea level, 9 km under the Moon's),
/// so terrain wins up close. The body itself is drawn at its full radius.
pub const IMPOSTOR_SCALE: f64 = 0.995;

/// Width of the full-detail maps, texels (height is half).
pub const FULL_MAP_WIDTH: u32 = 2048;
/// Width of the quick maps every visible body gets at once.
pub const SMALL_MAP_WIDTH: u32 = 256;
/// Texels across the ring profile.
pub const RING_MAP_WIDTH: u32 = 4096;

/// Surface pattern of a body seen from afar. The named looks are detailed portraits of the
/// solar system's bodies; the generic ones (`Rocky`, `Clouds`, `Banded`) take the body's color
/// for made-up worlds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum BodyLook {
    /// Not drawn (stars: the sky draws the sun).
    None,
    /// Cratered rock in the body's color, with darker plains.
    #[default]
    Rocky,
    /// Oceans, continents, deserts, forests, ice caps, and clouds with storms (Earth).
    Earthlike,
    /// A bright cloud deck in the body's color with faint streaks (Venus, or Titan with an
    /// orange color and haze).
    Clouds,
    /// Cloud bands cycling through `colors` from pole to pole (`bands` of them), with an
    /// optional great storm of this color.
    Banded {
        colors: Vec<[f32; 3]>,
        bands: f32,
        storm: Option<[f32; 3]>,
    },
    /// The Moon: maria on the near side (longitude 0 faces the parent), ray craters.
    Moon,
    /// Mercury: cratered, Caloris basin, bright rays.
    Mercury,
    /// Mars: dark albedo features, Tharsis, Valles Marineris, polar caps.
    Mars,
    /// Jupiter: belts and zones, festoons, white ovals, the Great Red Spot.
    Jupiter,
    /// Saturn: soft butterscotch bands and the polar hexagon.
    Saturn,
    /// Uranus: pale cyan, almost featureless.
    Uranus,
    /// Neptune: deep blue, a dark spot and bright streaks of high cloud.
    Neptune,
}

/// What the ground is like at one point of a body, as whoever builds it up close sees it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GroundPoint {
    /// Height above sea level, meters (negative under the sea).
    pub height_m: f32,
    /// 0 arid .. 1 lush.
    pub moisture: f32,
    /// Mean temperature above the one where snow and ice lie, °C (negative: white).
    pub above_frost_c: f32,
    /// Mean temperature, °C: the sea freezes over below about −9 °C, as the terrain freezes
    /// it.
    pub temperature_c: f32,
}

/// The ground another module builds for a body up close (a terrain), for the far view to
/// paint from: put it on the body's entity, and an Earth-like look draws that ground's
/// continents, seas, mountains, deserts, woods and snow instead of its own. The planet then
/// keeps its face as a ship closes in and the ground takes over (Kerbal Space Program exports
/// its scaled-space maps from its terrain the same way).
///
/// `sample(direction, texel_m)` is called once per map texel, on background threads, with a
/// body-local unit direction (+Y north, longitude 0 toward +Z, east toward +X) and the
/// texel's size on the ground (the finest detail worth computing).
#[derive(Component, Clone)]
pub struct FarGround(pub std::sync::Arc<dyn Fn(Vec3, f32) -> GroundPoint + Send + Sync>);

impl FarGround {
    pub fn new(sample: impl Fn(Vec3, f32) -> GroundPoint + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(sample))
    }

    /// Identity of the sampler (a new one rebakes the maps).
    pub(crate) fn id(&self) -> usize {
        std::sync::Arc::as_ptr(&self.0) as *const () as usize
    }
}

impl std::fmt::Debug for FarGround {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FarGround(..)")
    }
}

/// Marks a body whose ground another module draws (a terrain covers the far view's disk at
/// any distance). The far view then keeps the body's cloud tops as a layer of their own over
/// that ground from every distance, not only from low orbit.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct GroundDrawn;

/// How light falls on a body (see [`BodyLook::shading`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LookShading {
    /// Minnaert exponent: 1 = Lambert; below 1 darkens toward the limb (gas giants, clouds).
    pub minnaert: f32,
    /// Weight of the Lunar-Lambert (Lommel-Seeliger) term: 1 = a dusty airless body that
    /// stays bright out to the limb (the full Moon is a flat disk).
    pub lunar: f32,
    /// Exaggeration of the baked relief at the terminator (0 = none).
    pub relief: f32,
    /// Strength of the sun glint on water (Earth).
    pub glint: f32,
}

impl BodyLook {
    /// The lighting model the look is drawn with.
    pub fn shading(&self) -> LookShading {
        let s = |minnaert, lunar, relief, glint| LookShading {
            minnaert,
            lunar,
            relief,
            glint,
        };
        match self {
            BodyLook::None | BodyLook::Rocky => s(1.0, 0.7, 2.0, 0.0),
            BodyLook::Moon => s(1.0, 0.85, 2.5, 0.0),
            BodyLook::Mercury => s(1.0, 0.8, 2.5, 0.0),
            BodyLook::Mars => s(0.95, 0.35, 1.6, 0.0),
            BodyLook::Earthlike => s(1.0, 0.1, 0.0, 1.0),
            BodyLook::Clouds => s(0.8, 0.0, 0.0, 0.0),
            BodyLook::Banded { .. } | BodyLook::Jupiter | BodyLook::Saturn => {
                s(0.85, 0.0, 0.0, 0.0)
            }
            BodyLook::Uranus => s(0.78, 0.0, 0.0, 0.0),
            BodyLook::Neptune => s(0.8, 0.0, 0.0, 0.0),
        }
    }

    /// True for looks with a cloud layer over the ground (drawn as its own layer from high
    /// up, over the terrain).
    pub fn clouds(&self) -> bool {
        matches!(self, BodyLook::Earthlike)
    }

    /// True for looks with a solid surface (a terrain may cover them up close).
    pub fn solid(&self) -> bool {
        matches!(
            self,
            BodyLook::Rocky
                | BodyLook::Earthlike
                | BodyLook::Moon
                | BodyLook::Mercury
                | BodyLook::Mars
        )
    }
}

/// Haze or a thin atmosphere seen from afar: a glowing rim on the day side, a softer, redder
/// terminator, and a veil over the disk toward the limb.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyHaze {
    /// Vertical optical depth at the surface for the channel with `extinction` 1.
    pub optical_depth: f32,
    /// Relative extinction per channel (linear RGB): blue-heavy for clean air (Rayleigh),
    /// even for dust and smog.
    pub extinction: [f32; 3],
    /// Single-scattering albedo per channel: the color of the lit haze.
    pub albedo: [f32; 3],
    /// Density scale height, meters.
    pub scale_height_m: f64,
    /// Forward scattering (Henyey-Greenstein g, 0 = even, 0.7 = dust and droplets).
    pub forward: f32,
}

impl Default for BodyHaze {
    /// Earth-like clean air.
    fn default() -> Self {
        Self {
            optical_depth: 0.32,
            extinction: [0.18, 0.42, 1.0],
            albedo: [1.0, 1.0, 1.0],
            scale_height_m: 8_000.0,
            forward: 0.2,
        }
    }
}

/// A ring system in the body's equatorial plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyRings {
    pub inner_m: f64,
    pub outer_m: f64,
    /// Linear RGB of the densest ring material; thinner parts are grayer and darker.
    pub color: [f32; 3],
    /// Parts as (from, to, opacity) in meters from the center: opacity is seen face-on (0..1).
    /// Later entries override earlier ones where they overlap (gaps inside rings). Fine
    /// ringlets are added on top. Outside every part the rings are empty.
    pub bands: Vec<(f64, f64, f32)>,
}

impl Default for BodyRings {
    /// Saturn's main rings (Cassini radio occultation profiles, simplified): the faint C ring
    /// with its plateaus and the Colombo and Maxwell gaps, the dense B ring, the Cassini
    /// division with the Huygens gap, the A ring with the Encke and Keeler gaps, and the
    /// narrow F ring.
    fn default() -> Self {
        Self {
            inner_m: 74_490_000.0,
            outer_m: 140_700_000.0,
            color: [0.72, 0.6, 0.45],
            bands: vec![
                // C ring.
                (74_490_000.0, 84_000_000.0, 0.10),
                (77_750_000.0, 77_930_000.0, 0.02),
                (84_000_000.0, 91_980_000.0, 0.14),
                (84_750_000.0, 85_650_000.0, 0.32),
                (86_400_000.0, 86_600_000.0, 0.30),
                (87_290_000.0, 87_590_000.0, 0.03),
                (87_480_000.0, 87_540_000.0, 0.5),
                (88_400_000.0, 89_100_000.0, 0.3),
                (89_800_000.0, 90_600_000.0, 0.33),
                (91_000_000.0, 91_500_000.0, 0.28),
                // B ring: B1, B2, the opaque B3 core, B4.
                (91_980_000.0, 99_000_000.0, 0.72),
                (99_000_000.0, 104_500_000.0, 0.9),
                (104_500_000.0, 110_000_000.0, 0.98),
                (110_000_000.0, 117_580_000.0, 0.9),
                // Cassini division, its ringlets and the ramp up to the A ring.
                (117_580_000.0, 122_050_000.0, 0.08),
                (117_580_000.0, 117_750_000.0, 0.02),
                (117_790_000.0, 117_820_000.0, 0.6),
                (117_830_000.0, 118_190_000.0, 0.02),
                (118_550_000.0, 118_750_000.0, 0.16),
                (119_900_000.0, 120_100_000.0, 0.14),
                (120_700_000.0, 122_050_000.0, 0.22),
                // A ring with the Encke and Keeler gaps.
                (122_050_000.0, 133_423_000.0, 0.62),
                (126_000_000.0, 131_000_000.0, 0.55),
                (133_423_000.0, 133_745_000.0, 0.02),
                (133_745_000.0, 136_485_000.0, 0.48),
                (136_485_000.0, 136_527_000.0, 0.03),
                (136_527_000.0, 136_775_000.0, 0.45),
                // F ring.
                (140_150_000.0, 140_250_000.0, 0.35),
            ],
        }
    }
}

/// Opacity (0..1, seen face-on) of `rings` at `r` meters from the center: the band it falls
/// in (later bands win), with fine ringlets on top.
pub fn ring_opacity(rings: &BodyRings, r: f64) -> f32 {
    if r < rings.inner_m || r > rings.outer_m {
        return 0.0;
    }
    let Some(base) = rings
        .bands
        .iter()
        .rev()
        .find(|(a, b, _)| r >= *a && r < *b)
        .map(|b| b.2)
    else {
        return 0.0;
    };
    // Ringlets and density waves: smooth random structure at 2000, 450 and 90 km, deeper in
    // the thinner rings (the opaque B ring core stays opaque).
    let km = r / 1000.0;
    let wiggle = |scale: f64, seed: u32| -> f32 {
        let x = km / scale;
        let i = x.floor();
        let f = (x - i) as f32;
        let a = noise::unit(noise::hash3(i as i32, 0, 0, seed), 0);
        let b = noise::unit(noise::hash3(i as i32 + 1, 0, 0, seed), 0);
        (a + (b - a) * f * f * (3.0 - 2.0 * f)) * 2.0 - 1.0
    };
    let depth = 0.45 * (1.0 - base).max(0.15);
    let k =
        1.0 + depth * (0.5 * wiggle(2000.0, 1) + 0.35 * wiggle(450.0, 2) + 0.25 * wiggle(90.0, 3));
    (base * k).clamp(0.0, 1.0)
}

/// Linear color of the ring particles at `r`: the dense rings in `rings.color`, the thin ones
/// grayer and darker (as Saturn's C ring and Cassini division are).
pub fn ring_color(rings: &BodyRings, r: f64) -> [f32; 3] {
    let a = ring_opacity(rings, r);
    let k = (a / 0.9).clamp(0.0, 1.0);
    let gray = (rings.color[0] + rings.color[1] + rings.color[2]) / 3.0;
    let thin = [gray * 0.8, gray * 0.78, gray * 0.74];
    let span = ((r - rings.inner_m) / (rings.outer_m - rings.inner_m)) as f32;
    // Slightly warmer in the middle (the B ring), cooler toward the outer A ring.
    let warm = 1.0 + 0.06 * (1.0 - (span - 0.4).abs() * 2.5).max(0.0);
    let c = color::mix(thin, rings.color, k.sqrt());
    [c[0] * warm, c[1], c[2] / warm]
}

/// The far-view shape of a body (a child of the body entity): the spheroid, or its rings.
#[derive(Component, Debug, Clone, Copy)]
pub struct BodyImpostor {
    /// Radius of the drawn shape at scale 1, meters (the depth sphere for the body, the outer
    /// edge for rings).
    pub radius_m: f64,
}

/// Which part of a body an impostor draws.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImpostorPart {
    Body,
    Rings,
}

/// Marks bodies whose impostor has been created (or was not wanted).
#[derive(Component)]
pub(crate) struct ImpostorSpawned;

/// Bake progress and timings of the far-view maps (written by [`BodiesFeature`]).
#[derive(Resource, Debug, Clone, Default)]
pub struct LookBakes {
    /// Full-detail bakes waiting or running.
    pub pending: usize,
    /// Finished bakes: body name, map width, seconds.
    pub done: Vec<(String, u32, f32)>,
}

impl LookBakes {
    /// Seconds spent baking full-detail maps so far.
    pub fn total_seconds(&self) -> f32 {
        self.done
            .iter()
            .filter(|(_, w, _)| *w >= FULL_MAP_WIDTH)
            .map(|d| d.2)
            .sum()
    }
}

pub(crate) fn spawn_impostors(
    mut commands: Commands,
    settings: Res<SpaceSettings>,
    bodies: Query<(Entity, &CelestialBody), Without<ImpostorSpawned>>,
) {
    if !settings.draw_bodies {
        return;
    }
    for (e, body) in &bodies {
        commands.entity(e).insert(ImpostorSpawned);
        // Stars are drawn glowing (all but the one lighting the scene, which the sky draws).
        if body.kind != BodyKind::Star && body.look == BodyLook::None {
            continue;
        }
        commands.spawn((
            Transform::default(),
            BodyImpostor {
                radius_m: body.radius_m * IMPOSTOR_SCALE,
            },
            ImpostorPart::Body,
            ChildOf(e),
        ));
        if let Some(rings) = &body.rings {
            commands.spawn((
                Transform::default(),
                BodyImpostor {
                    radius_m: rings.outer_m,
                },
                ImpostorPart::Rings,
                ChildOf(e),
            ));
        }
    }
}

/// Planets and moons farther than this (m) are not enlarged into dots: they are drawn at
/// their true, sub-pixel size, so a planet of another star doesn't shine beside its star the
/// way Jupiter shines in our sky (at 2000 AU even Jupiter would be far too faint to see).
pub const MAX_DOT_DISTANCE_M: f64 = 2_000.0 * crate::orbit::AU;

/// Keep far bodies at least `min_apparent_radius_rad` wide as seen from the active camera
/// (out to [`MAX_DOT_DISTANCE_M`]).
pub(crate) fn scale_impostors(
    settings: Res<SpaceSettings>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    bodies: Query<&Transform, (With<CelestialBody>, Without<BodyImpostor>)>,
    mut impostors: Query<(&ChildOf, &BodyImpostor, &mut Transform), Without<CelestialBody>>,
) {
    let Some(eye) = cameras
        .iter()
        .filter(|(c, _)| c.active)
        .max_by_key(|(c, _)| c.order)
        .map(|(_, gt)| gt.translation())
    else {
        return;
    };
    for (parent, imp, mut tf) in &mut impostors {
        let Ok(body) = bodies.get(parent.parent()) else {
            continue;
        };
        let distance = (body.translation - eye).length();
        let scale = if distance > MAX_DOT_DISTANCE_M {
            1.0
        } else {
            (settings.min_apparent_radius_rad * distance / imp.radius_m).max(1.0) as f32
        };
        if (tf.scale.x - scale).abs() > scale * 1e-3 {
            tf.scale = Vec3::splat(scale);
        }
    }
}

/// Equatorial and polar radius of a body with `flattening` (mean radius kept).
pub fn spheroid_radii(radius_m: f64, flattening: f64) -> (f64, f64) {
    let f = flattening.clamp(0.0, 0.5);
    let eq = radius_m / (1.0 - f).cbrt();
    (eq, eq * (1.0 - f))
}

/// Unit direction of a latitude/longitude (degrees) in f32, for tests and tools.
pub fn look_dir(lat_deg: f32, lon_deg: f32) -> Vec3 {
    noise::dir(lat_deg.to_radians(), lon_deg.to_radians())
}

/// Luminance (cd/m²) of a lava world's exposed melt, where its maps say melt shows: a black
/// body at the melt's temperature (basalt at about 1400 K, hotter where the surface is: under
/// the star on a locked world, √2 times the mean). 0 for every other body.
pub fn melt_nits(body: &CelestialBody) -> f32 {
    match &body.surface {
        Some(r) if r.class == crate::nearby::WorldClass::Lava => {
            let hottest = r.temperature_k
                * if body.tidally_locked {
                    2f64.sqrt()
                } else {
                    1.05
                };
            black_body_luminance(hottest.max(1400.0)) as f32
        }
        _ => 0.0,
    }
}

/// Luminance (cd/m²) of a black body at `kelvin`: Planck's law weighted by the eye's photopic
/// response (a Gaussian fit to CIE V(λ)), 683 lm/W at its peak.
pub fn black_body_luminance(kelvin: f64) -> f64 {
    const H: f64 = 6.626_070_15e-34;
    const C: f64 = 2.997_924_58e8;
    const K: f64 = 1.380_649e-23;
    let mut sum = 0.0;
    let step = 5e-9;
    let mut l: f64 = 380e-9;
    while l <= 780e-9 {
        let planck = 2.0 * H * C * C / l.powi(5) / ((H * C / (l * K * kelvin)).exp() - 1.0);
        let um = l * 1e6;
        let v = 1.019 * (-285.4 * (um - 0.559) * (um - 0.559)).exp();
        sum += planck * v * step;
        l += step;
    }
    683.0 * sum
}
