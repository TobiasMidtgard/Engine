//! How bodies look from afar. Every body that nothing else draws gets a textured sphere (and
//! Saturn its rings) as a child, so Jupiter's bands, Saturn's rings and the blue Earth show
//! when you fly out. The sphere sits a little inside the body's radius, so a terrain on the
//! same body always covers it up close. Far away it is kept at least a few pixels wide, so
//! planets shine as dots in the night sky the way they do in reality.

use engine_assets::Assets;
use engine_core::math::noise::fbm3;
use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::*;

use crate::{BodyKind, CelestialBody, SpaceSettings};

/// The far-view sphere is this fraction of the body's radius (below any terrain's lowest
/// point: 32 km under Earth's sea level, 9 km under the Moon's).
pub const IMPOSTOR_SCALE: f64 = 0.995;

/// Surface pattern of a body seen from afar.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub enum BodyLook {
    /// Not drawn (stars: the sky draws the sun).
    None,
    /// Mottled rock or dust in the body's color, with darker plains.
    #[default]
    Rocky,
    /// Oceans, continents, deserts, ice caps and clouds.
    Earthlike,
    /// A bright cloud deck with faint swirls (Venus).
    Clouds,
    /// Cloud bands cycling through `colors` from pole to pole (`bands` of them), with an
    /// optional great storm of this color.
    Banded {
        colors: Vec<[f32; 3]>,
        bands: f32,
        storm: Option<[f32; 3]>,
    },
}

/// A ring system in the body's equatorial plane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BodyRings {
    pub inner_m: f64,
    pub outer_m: f64,
    /// Linear RGB.
    pub color: [f32; 3],
    /// Dense parts as (from, to, opacity) in meters from the center; the rest is faint.
    pub bands: Vec<(f64, f64, f32)>,
}

impl Default for BodyRings {
    /// Saturn's main rings: the faint C ring, the bright B ring, the Cassini division and the A
    /// ring with the Encke gap.
    fn default() -> Self {
        Self {
            inner_m: 74_500_000.0,
            outer_m: 136_800_000.0,
            color: [0.78, 0.7, 0.55],
            bands: vec![
                (74_500_000.0, 92_000_000.0, 0.25),
                (92_000_000.0, 117_580_000.0, 0.95),
                (117_580_000.0, 122_170_000.0, 0.08),
                (122_170_000.0, 133_400_000.0, 0.7),
                (133_400_000.0, 133_750_000.0, 0.05),
                (133_750_000.0, 136_800_000.0, 0.6),
            ],
        }
    }
}

/// The far-view sphere or rings of a body (a child of the body entity).
#[derive(Component, Debug, Clone, Copy)]
pub struct BodyImpostor {
    /// Radius of the drawn shape at scale 1, meters.
    pub radius_m: f64,
}

/// Marks bodies whose impostor has been created (or was not wanted).
#[derive(Component)]
pub(crate) struct ImpostorSpawned;

pub(crate) fn spawn_impostors(
    mut commands: Commands,
    settings: Res<SpaceSettings>,
    bodies: Query<(Entity, &CelestialBody), Without<ImpostorSpawned>>,
    mut images: Option<ResMut<Assets<Image>>>,
) {
    if !settings.draw_bodies {
        return;
    }
    for (e, body) in &bodies {
        commands.entity(e).insert(ImpostorSpawned);
        if body.kind == BodyKind::Star || body.look == BodyLook::None {
            continue;
        }
        let radius = body.radius_m * IMPOSTOR_SCALE;
        let mut material = StandardMaterial::color(body.color[0], body.color[1], body.color[2])
            .with_roughness(0.95);
        if let Some(images) = images.as_mut() {
            material.base_color = [1.0; 4];
            material = material.with_base_color_texture(images.add(surface_texture(body)));
        }
        let mut sphere = MeshRenderer::new(
            Shape::Sphere {
                radius: radius as f32,
                segments: 96,
            },
            material,
        );
        sphere.cast_shadows = false;
        commands.spawn((
            Transform::default(),
            sphere,
            BodyImpostor { radius_m: radius },
            ChildOf(e),
        ));
        if let Some(rings) = &body.rings {
            let mut material =
                StandardMaterial::color(rings.color[0], rings.color[1], rings.color[2])
                    .with_roughness(0.9)
                    .with_alpha(1.0);
            material.double_sided = true;
            material.subsurface = 0.5;
            if let Some(images) = images.as_mut() {
                material.base_color = [1.0; 4];
                material = material.with_base_color_texture(images.add(ring_texture(rings)));
            }
            let size = (rings.outer_m * 2.0) as f32;
            let mut quad = MeshRenderer::new(Shape::Quad { size: [size, size] }, material);
            quad.cast_shadows = false;
            commands.spawn((
                // The quad faces +Z; lay it in the equatorial plane (+Y is the body's pole).
                Transform::from_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
                quad,
                BodyImpostor {
                    radius_m: rings.outer_m,
                },
                ChildOf(e),
            ));
        }
    }
}

/// Keep far bodies at least `min_apparent_radius_rad` wide as seen from the active camera.
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
        let scale = (settings.min_apparent_radius_rad * distance / imp.radius_m).max(1.0) as f32;
        if (tf.scale.x - scale).abs() > scale * 1e-3 {
            tf.scale = Vec3::splat(scale);
        }
    }
}

// ---- Procedural textures (equirectangular: row 0 is the north pole, +Y).

const TEX_W: u32 = 512;
const TEX_H: u32 = 256;

fn srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn scale3(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Unit direction of texel (x, y), matching the renderer's UV sphere.
fn texel_dir(x: u32, y: u32) -> DVec3 {
    let theta = (x as f64 + 0.5) / TEX_W as f64 * std::f64::consts::TAU;
    let phi = (y as f64 + 0.5) / TEX_H as f64 * std::f64::consts::PI;
    DVec3::new(phi.sin() * theta.cos(), phi.cos(), -phi.sin() * theta.sin())
}

fn noise(d: DVec3, freq: f64, seed: u64, octaves: u32) -> f32 {
    fbm3(d.x * freq, d.y * freq, d.z * freq, seed, octaves, 2.0, 0.5)
}

fn seed_of(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// The surface pattern of `body` as a linear color at direction `d`.
pub fn surface_color(body: &CelestialBody, d: DVec3) -> [f32; 3] {
    let seed = seed_of(&body.name);
    let lat = d.y.clamp(-1.0, 1.0).asin() as f32;
    match &body.look {
        BodyLook::None | BodyLook::Rocky => {
            let mottle = noise(d, 6.0, seed, 6);
            let plains = smooth(0.05, 0.2, noise(d, 1.6, seed ^ 1, 4));
            let c = scale3(body.color, 0.8 + 0.5 * mottle);
            mix3(c, scale3(body.color, 0.55), plains * 0.7)
        }
        BodyLook::Earthlike => {
            let land = noise(d, 1.8, seed, 7) + 0.04 * noise(d, 12.0, seed ^ 2, 3);
            let ocean = [0.012, 0.035, 0.11];
            let dry = smooth(
                0.0,
                0.35,
                0.3 - (lat.abs() - 0.42).abs() + noise(d, 3.0, seed ^ 3, 4),
            );
            let ground = mix3([0.05, 0.11, 0.035], [0.32, 0.26, 0.15], dry);
            let mut c = if land > 0.06 { ground } else { ocean };
            let ice = smooth(1.15, 1.25, lat.abs() + 0.08 * noise(d, 5.0, seed ^ 4, 3));
            c = mix3(c, [0.85, 0.88, 0.92], ice);
            let cloud = smooth(
                0.08,
                0.3,
                noise(d, 4.0, seed ^ 5, 6) + 0.15 * (lat * 3.0).cos() - 0.12,
            );
            mix3(c, [0.9, 0.9, 0.92], cloud * 0.85)
        }
        BodyLook::Clouds => {
            let swirl = noise(d * DVec3::new(1.0, 3.0, 1.0), 3.0, seed, 5);
            scale3(body.color, 0.92 + 0.12 * swirl)
        }
        BodyLook::Banded {
            colors,
            bands,
            storm,
        } => {
            if colors.is_empty() {
                return body.color;
            }
            // Bands squeeze toward the equator and wave along their edges.
            let turb = noise(d, 5.0, seed, 5) * 0.3 + noise(d, 18.0, seed ^ 7, 3) * 0.08;
            let s = lat.sin();
            let t = (s * (1.0 + 0.35 * (1.0 - s.abs())) * 0.5 + 0.5) * bands + turb;
            let i = t.floor() as i64;
            let f = smooth(0.25, 0.75, t - t.floor());
            let n = colors.len() as i64;
            let a = colors[i.rem_euclid(n) as usize];
            let b = colors[(i + 1).rem_euclid(n) as usize];
            // Fine streaks within the bands.
            let streak = 0.94 + 0.06 * (lat * 70.0 + turb * 9.0).sin();
            let mut c = scale3(mix3(a, b, f), streak);
            if let Some(color) = storm {
                // A great oval storm at 22° south, with a swirl around it.
                let lon = (-d.z).atan2(d.x) as f32 - 0.8;
                let (dx, dy) = (lon * 0.55, (lat + 0.384) * 2.2);
                let r = (dx * dx + dy * dy).sqrt() * 9.0;
                let spot = 1.0 - smooth(0.55, 1.0, r + 0.15 * noise(d, 30.0, seed ^ 9, 2));
                let collar = (1.0 - smooth(1.0, 1.4, r)) * (1.0 - spot);
                c = mix3(c, scale3(c, 1.15), collar);
                c = mix3(c, *color, spot * 0.85);
            }
            c
        }
    }
}

fn surface_texture(body: &CelestialBody) -> Image {
    let mut data = Vec::with_capacity((TEX_W * TEX_H * 4) as usize);
    for y in 0..TEX_H {
        for x in 0..TEX_W {
            let c = surface_color(body, texel_dir(x, y));
            data.extend_from_slice(&[srgb(c[0]), srgb(c[1]), srgb(c[2]), 255]);
        }
    }
    Image::new(TEX_W, TEX_H, data, true)
}

/// Opacity (0..1) of `rings` at `r` meters from the center.
pub fn ring_opacity(rings: &BodyRings, r: f64) -> f32 {
    if r < rings.inner_m || r > rings.outer_m {
        return 0.0;
    }
    let base = rings
        .bands
        .iter()
        .find(|(a, b, _)| r >= *a && r < *b)
        .map_or(0.15, |b| b.2);
    // Ringlets, at spacings the texture can hold (a texel is ~0.5 Mm on Saturn).
    let ringlets =
        0.8 + 0.12 * (r / 1_300_000.0).sin() as f32 + 0.08 * (r / 2_900_000.0 + 1.3).sin() as f32;
    (base * ringlets).clamp(0.0, 1.0)
}

fn ring_texture(rings: &BodyRings) -> Image {
    const N: u32 = 512;
    let mut data = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let u = (x as f64 + 0.5) / N as f64 * 2.0 - 1.0;
            let v = (y as f64 + 0.5) / N as f64 * 2.0 - 1.0;
            let r = (u * u + v * v).sqrt() * rings.outer_m;
            let a = ring_opacity(rings, r);
            let tint = 0.8 + 0.25 * ((r - rings.inner_m) / (rings.outer_m - rings.inner_m)) as f32;
            let c = scale3(rings.color, tint);
            // Alpha is linear coverage, whatever the color space.
            data.extend_from_slice(&[srgb(c[0]), srgb(c[1]), srgb(c[2]), (a * 255.0) as u8]);
        }
    }
    Image::new(N, N, data, true)
}
