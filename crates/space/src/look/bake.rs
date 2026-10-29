//! Baking body textures on the CPU: an equirectangular albedo map (sRGB, alpha = water for the
//! sun glint) and an auxiliary map (linear: cloud cover for Earth, surface slopes for relief
//! on rocky bodies, glowing melt on lava worlds), both with full mip chains, plus the radial profile of a ring system.
//!
//! Maps follow planetary map conventions: longitude −180° at the left edge, 0° in the middle,
//! east to the right; row 0 is the north pole. Longitude 0 points along the body's +Z, east
//! along +X (as [`crate::dir_from_lat_lon`]).

use std::sync::Mutex;

use engine_core::math::Vec3;
use engine_render::Image;

use super::{BodyLook, BodyRings, FarGround};
use crate::CelestialBody;

/// Worker threads used for one bake.
pub(crate) fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 16)
}

/// Run `f(row_index, row)` over the rows of `data` (`width` items each) on all cores. Rows are
/// handed out a few at a time, so expensive regions (storms, ray craters) balance out.
pub(crate) fn par_rows<T: Send>(data: &mut [T], width: usize, f: impl Fn(usize, &mut [T]) + Sync) {
    const ROWS: usize = 4;
    let width = width.max(1);
    let n = threads();
    if n == 1 || data.len() <= width * ROWS {
        for (y, row) in data.chunks_mut(width).enumerate() {
            f(y, row);
        }
        return;
    }
    let chunks = Mutex::new(data.chunks_mut(width * ROWS).enumerate());
    let f = &f;
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    let next = chunks.lock().map(|mut c| c.next()).ok().flatten();
                    let Some((i, chunk)) = next else { break };
                    for (k, row) in chunk.chunks_mut(width).enumerate() {
                        f(i * ROWS + k, row);
                    }
                }
            });
        }
    });
}

/// One texel to paint: its direction on the unit sphere, its latitude/longitude (radians) and
/// its column and row in the map being baked (for fields the painter splatted at that size).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Texel {
    pub d: Vec3,
    pub lat: f32,
    pub lon: f32,
    pub x: u32,
    pub y: u32,
}

/// What a painter returns for a texel.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Paint {
    /// Linear albedo.
    pub color: [f32; 3],
    /// 1 over open water (sun glint), 0 on land and clouds.
    pub water: f32,
    /// Height above the mean radius, in body radii (relief at the terminator).
    pub height: f32,
    /// Cloud cover 0..1 (a separate layer above the surface).
    pub cloud: f32,
    /// Where molten rock shows, 0..1 (it glows by its own light).
    pub glow: f32,
}

/// Paints one body's surface. Painters precompute what they need for a given map width.
pub(crate) trait Painter: Sync {
    fn paint(&self, t: &Texel) -> Paint;
    /// The height channel is meaningful (rocky bodies): bake slopes into the aux map.
    fn relief(&self) -> bool {
        false
    }
    /// The cloud channel is meaningful: bake it into the aux map.
    fn clouds(&self) -> bool {
        false
    }
}

/// The painter for a body's look, set up for maps `width` texels wide.
pub(crate) fn painter(
    body: &CelestialBody,
    ground: Option<&FarGround>,
    width: u32,
) -> Box<dyn Painter> {
    use super::{earth, gas, rock, world};
    let seed = seed_of(&body.name);
    // Planets of other stars are painted from what their surface is.
    if let Some(r) = &body.surface {
        if world::Barren::draws(r.class) && body.look == BodyLook::Rocky {
            return Box::new(world::Barren::new(
                r,
                body.color,
                body.tidally_locked,
                seed,
                width,
            ));
        }
        if body.look == BodyLook::Earthlike {
            let climate = earth::Climate {
                sea: Some(r.sea_fraction as f32),
                kelvin: r.temperature_k as f32,
                locked: body.tidally_locked,
                plants: Some((
                    earth::plants_under(r.star_kelvin as f32),
                    earth::straw_under(r.star_kelvin as f32),
                )),
                earth_poles: false,
                cloudiness: (r.cloud_cover - 0.55) * 0.6,
            };
            return Box::new(earth::Earth::with(seed, width, climate).on(
                ground,
                body.radius_m,
                width,
            ));
        }
    }
    match &body.look {
        BodyLook::None | BodyLook::Rocky => Box::new(rock::Rocky::new(body.color, seed, width)),
        BodyLook::Moon => Box::new(rock::Moon::new(seed, width)),
        BodyLook::Mercury => Box::new(rock::Mercury::new(seed, width)),
        BodyLook::Mars => Box::new(rock::Mars::new(seed, width)),
        BodyLook::Earthlike => {
            Box::new(earth::Earth::new(seed, width).on(ground, body.radius_m, width))
        }
        BodyLook::Clouds => Box::new(gas::Venus::new(body.color, seed, width)),
        BodyLook::Banded {
            colors,
            bands,
            storm,
        } => Box::new(gas::Banded::new(
            colors.clone(),
            *bands,
            *storm,
            body.color,
            seed,
            width,
        )),
        BodyLook::Jupiter => Box::new(gas::Jupiter::new(seed, width)),
        BodyLook::Saturn => Box::new(gas::Saturn::new(seed, width)),
        BodyLook::Uranus => Box::new(gas::Uranus::new(seed)),
        BodyLook::Neptune => Box::new(gas::Neptune::new(seed, width)),
    }
}

/// FNV-1a of a name: the default seed of a body's patterns.
pub(crate) fn seed_of(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x100_0000_01b3)
    })
}

/// The baked maps of one body.
#[derive(Clone)]
pub struct SurfaceMaps {
    /// sRGB albedo; alpha is 255 over open water (sun glint), 0 elsewhere. With mips.
    pub albedo: Image,
    /// Linear data: R = cloud cover, G/B = east/north surface slope (0.5 = flat), A = glowing
    /// melt.
    /// With mips.
    pub aux: Image,
    /// The aux map holds clouds / relief.
    pub clouds: bool,
    pub relief: bool,
    /// Wall-clock bake time, seconds.
    pub seconds: f32,
}

/// Slopes are stored as `0.5 + slope * SLOPE_SCALE` (a slope of 1 is 45°).
pub const SLOPE_SCALE: f32 = 0.25;

fn srgb_lut() -> &'static [u8; 4096] {
    static LUT: std::sync::OnceLock<[u8; 4096]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        std::array::from_fn(|i| {
            let l = i as f32 / 4095.0;
            let s = if l <= 0.003_130_8 {
                l * 12.92
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
        })
    })
}

/// Linear 0..1 to an sRGB byte.
#[inline]
pub(crate) fn to_srgb(v: f32) -> u8 {
    srgb_lut()[(v.clamp(0.0, 1.0) * 4095.0 + 0.5) as usize]
}

#[inline]
fn unorm(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Bake `body`'s maps at `width`×`width/2` texels (width is rounded up to a power of two).
pub fn bake_surface(body: &CelestialBody, width: u32) -> SurfaceMaps {
    bake_surface_on(body, None, width)
}

/// [`bake_surface`] over the ground another module builds for the body (see [`FarGround`]).
pub fn bake_surface_on(
    body: &CelestialBody,
    ground: Option<&FarGround>,
    width: u32,
) -> SurfaceMaps {
    let started = std::time::Instant::now();
    let width = width.clamp(16, 8192).next_power_of_two();
    let height = width / 2;
    let painter = painter(body, ground, width);
    let (relief, clouds) = (painter.relief(), painter.clouds());

    #[derive(Clone, Copy, Default)]
    struct Out {
        albedo: [u8; 4],
        cloud: u8,
        glow: u8,
        height: f32,
    }
    let mut out = vec![Out::default(); (width * height) as usize];
    // Longitudes and their sines and cosines, shared by every row.
    let cols: Vec<(f32, f32, f32)> = (0..width)
        .map(|x| {
            let lon =
                (x as f32 + 0.5) / width as f32 * std::f32::consts::TAU - std::f32::consts::PI;
            let (s, c) = lon.sin_cos();
            (lon, s, c)
        })
        .collect();
    par_rows(&mut out, width as usize, |y, row| {
        let lat =
            std::f32::consts::FRAC_PI_2 - (y as f32 + 0.5) / height as f32 * std::f32::consts::PI;
        let (sl, cl) = lat.sin_cos();
        for (x, o) in row.iter_mut().enumerate() {
            let (lon, so, co) = cols[x];
            let p = painter.paint(&Texel {
                d: Vec3::new(cl * so, sl, cl * co),
                lat,
                lon,
                x: x as u32,
                y: y as u32,
            });
            o.albedo = [
                to_srgb(p.color[0]),
                to_srgb(p.color[1]),
                to_srgb(p.color[2]),
                unorm(p.water),
            ];
            o.cloud = unorm(p.cloud);
            o.glow = unorm(p.glow);
            o.height = p.height;
        }
    });

    let mut aux = vec![[0u8, 128, 128, 0]; out.len()];
    let w = width as usize;
    let h = height as usize;
    par_rows(&mut aux, w, |y, row| {
        // Texel spacing in radians: east (shrinks toward the poles) and north.
        let step = std::f32::consts::TAU / width as f32;
        let lat = std::f32::consts::FRAC_PI_2 - (y as f32 + 0.5) * step;
        let east_step = step * lat.cos().max(0.02);
        for (x, a) in row.iter_mut().enumerate() {
            let i = y * w + x;
            a[0] = out[i].cloud;
            a[3] = out[i].glow;
            if relief {
                let left = out[y * w + (x + w - 1) % w].height;
                let right = out[y * w + (x + 1) % w].height;
                let up = out[y.saturating_sub(1) * w + x].height;
                let down = out[(y + 1).min(h - 1) * w + x].height;
                let se = (right - left) / (2.0 * east_step);
                let sn = (up - down) / (2.0 * step);
                a[1] = unorm(0.5 + se * SLOPE_SCALE);
                a[2] = unorm(0.5 + sn * SLOPE_SCALE);
            }
        }
    });

    let albedo_bytes: Vec<u8> = out.iter().flat_map(|o| o.albedo).collect();
    drop(out);
    let aux_bytes: Vec<u8> = aux.into_iter().flatten().collect();
    let mut albedo = Image::new(width, height, albedo_bytes, true);
    let mut aux = Image::new(width, height, aux_bytes, false);
    std::thread::scope(|s| {
        s.spawn(|| albedo.generate_mips());
        s.spawn(|| aux.generate_mips());
    });
    SurfaceMaps {
        albedo,
        aux,
        clouds,
        relief,
        seconds: started.elapsed().as_secs_f32(),
    }
}

/// The linear albedo of `body`'s look at body-local unit direction `d` (+Y north, longitude 0
/// toward +Z, east toward +X), as the far view paints it. Builds the painter, so call it for
/// a few points (tests, tools), not per texel.
pub fn surface_color(body: &CelestialBody, d: Vec3) -> [f32; 3] {
    const WIDTH: u32 = 1024;
    let p = painter(body, None, WIDTH);
    let d = d.normalize_or(Vec3::Y);
    let (lat, lon) = super::noise::lat_lon(d);
    let (u, v) = super::noise::uv(lat, lon);
    let x = ((u * WIDTH as f32) as u32).min(WIDTH - 1);
    let y = ((v * (WIDTH / 2) as f32) as u32).min(WIDTH / 2 - 1);
    p.paint(&Texel { d, lat, lon, x, y }).color
}

/// Ring radial profile: `width` texels from `inner_m` (u = 0) to `outer_m` (u = 1). RGB is
/// the particles' color (sRGB), alpha the opacity seen face-on (1 − e^−τ). With mips.
pub fn bake_rings(rings: &BodyRings, width: u32) -> Image {
    let width = width.clamp(16, 16384).next_power_of_two();
    let mut data = Vec::with_capacity(width as usize * 4);
    for x in 0..width {
        let r = rings.inner_m + (x as f64 + 0.5) / width as f64 * (rings.outer_m - rings.inner_m);
        let a = super::ring_opacity(rings, r);
        let c = super::ring_color(rings, r);
        data.extend_from_slice(&[to_srgb(c[0]), to_srgb(c[1]), to_srgb(c[2]), unorm(a)]);
    }
    let mut image = Image::new(width, 1, data, true);
    image.generate_mips();
    image
}
