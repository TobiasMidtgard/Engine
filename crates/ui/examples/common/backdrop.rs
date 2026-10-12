//! The gallery's 3D backdrop: Hollow Reach, a frontier trading post on the highlands of Veyra,
//! at golden hour.
//!
//! Everything is generated in code (the gallery ships no asset files):
//!
//! - a camera-centred polar heightfield from the player's boots out to mountain ranges 25 km
//!   away (dense near the eye, coarse far away, so every triangle covers about the same number
//!   of pixels), coloured by slope, height and wear, with baked cavity occlusion;
//! - procedural PBR texture sets (albedo, tangent-space normal, occlusion/roughness/metal) for
//!   ground, worn soil, rock, wood, canvas and painted metal;
//! - tussock grass, boulders, Old Brannoc's stall (posts, canvas awning, counter, crates,
//!   barrels, goods and a lantern), a hab pod with a dish, a water tank and a wind turbine,
//!   way markers along the path, the ridge lookout, the relay tower with its warning light,
//!   and a moon;
//! - golden-hour light: a low sun from the left with long cascaded shadows across the path,
//!   atmosphere, valley haze, warm bounce light and fixed exposure;
//! - a small post pass (FXAA and screen-space ambient occlusion), which the renderer lacks,
//!   and a static-scene cache: once the view holds still, the finished frame is captured and
//!   replayed with its depth, so the gallery's hundreds of frames cost little (see [`PostFx`]).
//!
//! The UI's world markers point at the stall (6, 3, −6), the ridge lookout (60, 12, −182)
//! and the relay tower (−140, 40, −902); the camera stands at (0, 1.8, 4) looking down −Z.
//! The middle of the screen and its lower third (where the HUD prompt and dialogue sit) stay
//! calm: the path and open grass.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU};

use engine_assets::{Assets, Handle};
use engine_core::prelude::*;
use engine_render::features::{
    FrameContext, GpuShared, PendingRenderFeatures, RenderFeature, RenderPhase,
};
use engine_render::prelude::*;
use engine_render::{Mesh, Vertex, ViewData, wgpu};
use engine_world::WeatherState;

/// Eye position, metres.
const EYE: DVec3 = DVec3::new(0.0, 1.8, 4.0);
/// Point the camera looks at.
const LOOK_AT: DVec3 = DVec3::new(0.0, 1.62, -6.0);
/// Vertical field of view, degrees.
const FOV_Y_DEG: f32 = 60.0;
/// Old Brannoc's stall (ground position, x/z).
const STALL: Vec2 = Vec2::new(6.0, -6.0);
/// Which way the stall's front faces (yaw about +Y; 0 = toward +Z).
const STALL_YAW: f32 = -0.42;
/// The hab pod behind the stall.
const HAB: Vec2 = Vec2::new(-15.5, -27.0);
/// The ridge lookout on the spur's knoll.
const LOOKOUT: Vec2 = Vec2::new(60.0, -182.0);
/// The relay tower on the far ridge.
const TOWER: Vec2 = Vec2::new(-140.0, -902.0);
/// Sun direction: azimuth from −Z toward +X (negative = left of the view) and elevation.
const SUN_AZIMUTH_DEG: f32 = -80.0;
const SUN_ELEVATION_DEG: f32 = 8.0;
/// The planet's surface lies this far below world y = 0, so the valley floor (about −125 m)
/// is still above it (atmosphere and fog heights are altitudes above the surface).
const DATUM: f64 = 220.0;
/// Fixed exposure, EV100 (golden hour, sun low).
const EV100: f32 = 12.8;

/// Spawns the whole backdrop: render settings, atmosphere, sun, camera, terrain, props and
/// the post pass. Call after `RenderPlugin` was added and before the first frame.
pub fn spawn_backdrop(world: &mut World) {
    configure(world);
    let tex = Textures::generate(world);
    let mats = Materials::new(world, &tex);
    spawn_camera_and_lights(world);
    let terrain = spawn_terrain(world, &mats);
    spawn_path_decal(world, &mats);
    let rocks = spawn_rocks(world, &mats, &terrain);
    spawn_grass(world, &mats, &rocks);
    spawn_stall(world, &mats);
    spawn_hab(world, &mats);
    spawn_way_markers(world, &mats);
    spawn_lookout(world, &mats);
    spawn_tower(world, &mats);
    spawn_moon(world, &tex);
    world
        .resource_mut::<Schedules>()
        .add_systems(Update, blink_beacons);
    world
        .get_resource_or_init::<PendingRenderFeatures>()
        .0
        .push(Box::new(PostFx::default()));
}

// ============================================================================ settings

fn sun_dir() -> Vec3 {
    let (a, e) = (SUN_AZIMUTH_DEG.to_radians(), SUN_ELEVATION_DEG.to_radians());
    Vec3::new(a.sin() * e.cos(), e.sin(), -a.cos() * e.cos())
}

fn configure(world: &mut World) {
    {
        let mut s = world.resource_mut::<RenderSettings>();
        s.shadow_distance = 1600.0;
        s.shadow_map_size = 4096;
        s.shadow_split_lambda = 0.94;
        s.shadow_softness = 1.3;
        s.shadow_normal_bias = 1.2;
        s.bloom_intensity = 0.045;
        s.saturation = 1.06;
        s.contrast = 1.1;
        s.vignette = 0.26;
        s.film_grain = 0.005;
        s.white_balance = 0.1;
        // The sky never changes here: render the environment map once in a while only.
        s.environment_interval = 120;
        s.sky_occlusion_extent = 160.0;
    }
    *world.resource_mut::<Atmosphere>() = Atmosphere {
        planet_center: DVec3::new(0.0, -6_360_000.0 - DATUM, 0.0),
        planet_radius: 6_360_000.0,
        mie_scale_height: 900.0,
        mie_g: 0.78,
        // Golden grassland: warm light bounced into every shadow.
        ground_color: [0.24, 0.17, 0.09],
        ..Default::default()
    };
    *world.resource_mut::<FogSettings>() = FogSettings {
        density: 0.0045,
        height_falloff: 0.032,
        base_height: (DATUM - 125.0) as f32,
        max_opacity: 0.9,
        color: [1.0, 0.9, 0.8],
        sun_scattering: 0.9,
    };
    *world.resource_mut::<AmbientLight>() = AmbientLight {
        color: [0.75, 0.8, 1.0],
        brightness: 0.03,
    };
    world.insert_resource(WeatherState {
        cloud_cover: 0.0,
        cloud_density: 0.35,
        wind: Vec3::new(2.5, 0.0, -1.0),
        sun_direction: sun_dir(),
        time_of_day_h: 18.3,
        fog: 0.0,
        ..Default::default()
    });
}

fn spawn_camera_and_lights(world: &mut World) {
    world.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: EV100 },
            ..Camera::perspective(FOV_Y_DEG)
        },
        Transform::from_translation(EYE).looking_at(LOOK_AT, Vec3::Y),
    ));
    world.spawn((
        DirectionalLight {
            color: [1.0, 0.97, 0.92],
            illuminance: 120_000.0,
            cast_shadows: true,
            angular_size_deg: 0.6,
        },
        Transform::default().looking_at(-sun_dir().as_dvec3(), Vec3::Y),
        StaticBackdrop,
    ));
}

// ============================================================================ helpers

/// Run `f(i)` for `i` in `0..n` on all cores; results in order.
fn par_map<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 16);
    let chunk = n.div_ceil(threads).max(1);
    let f = &f;
    std::thread::scope(|s| {
        let jobs: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|start| {
                s.spawn(move || (start..(start + chunk).min(n)).map(f).collect::<Vec<T>>())
            })
            .collect();
        jobs.into_iter()
            .flat_map(|j| j.join().expect("backdrop worker"))
            .collect()
    })
}

fn sat(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = sat((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

fn mix3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t
}

/// Smooth maximum (polynomial), blending over `k`.
fn smax(a: f32, b: f32, k: f32) -> f32 {
    let h = sat(0.5 + 0.5 * (b - a) / k);
    a + (b - a) * h + k * h * (1.0 - h)
}

/// Smooth minimum with blend width `k`.
fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = sat(0.5 + 0.5 * (b - a) / k);
    b + (a - b) * h - k * h * (1.0 - h)
}

/// sRGB hex to linear RGB.
fn hex(rgb: u32) -> Vec3 {
    Vec3::from(engine_render::material::srgb_hex_to_linear(rgb))
}

fn rgba(c: Vec3) -> Vec4 {
    c.extend(1.0)
}

fn vtx(p: Vec3, n: Vec3, uv: Vec2, c: Vec4) -> Vertex {
    Vertex {
        position: p.to_array(),
        normal: n.to_array(),
        uv: uv.to_array(),
        color: c.to_array(),
    }
}

/// Flip triangles whose winding disagrees with their vertex normals.
fn fix_winding(m: &mut Mesh) {
    let verts = &m.vertices;
    for tri in m.indices.as_chunks_mut::<3>().0 {
        let p = |i: u32| Vec3::from(verts[i as usize].position);
        let n = |i: u32| Vec3::from(verts[i as usize].normal);
        let g = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
        if g.dot(n(tri[0]) + n(tri[1]) + n(tri[2])) < 0.0 {
            tri.swap(1, 2);
        }
    }
}

fn tint(mut m: Mesh, c: Vec4) -> Mesh {
    for v in &mut m.vertices {
        v.color = (Vec4::from(v.color) * c).to_array();
    }
    m
}

fn moved(mut m: Mesh, t: Mat4) -> Mesh {
    m.transform(&t);
    m
}

fn add_mesh(world: &mut World, m: Mesh) -> Handle<Mesh> {
    world.resource_mut::<Assets<Mesh>>().add(m)
}

fn add_mat(world: &mut World, m: StandardMaterial) -> Handle<StandardMaterial> {
    world.resource_mut::<Assets<StandardMaterial>>().add(m)
}

fn add_image(world: &mut World, i: Image) -> Handle<Image> {
    world.resource_mut::<Assets<Image>>().add(i)
}

fn spawn_mesh(
    world: &mut World,
    mesh: Mesh,
    mat: Handle<StandardMaterial>,
    t: Transform,
    shadows: bool,
) -> Entity {
    let h = add_mesh(world, mesh);
    spawn_handle(world, h, mat, t, shadows)
}

fn spawn_handle(
    world: &mut World,
    mesh: Handle<Mesh>,
    mat: Handle<StandardMaterial>,
    t: Transform,
    shadows: bool,
) -> Entity {
    let mut r = MeshRenderer::new(mesh, mat);
    r.cast_shadows = shadows;
    world.spawn((t, r, StaticBackdrop)).id()
}

// ============================================================================ noise

mod noise {
    use std::f32::consts::FRAC_1_SQRT_2 as H;

    const G2: [[f32; 2]; 16] = [
        [1.0, 0.0],
        [0.92388, 0.38268],
        [H, H],
        [0.38268, 0.92388],
        [0.0, 1.0],
        [-0.38268, 0.92388],
        [-H, H],
        [-0.92388, 0.38268],
        [-1.0, 0.0],
        [-0.92388, -0.38268],
        [-H, -H],
        [-0.38268, -0.92388],
        [0.0, -1.0],
        [0.38268, -0.92388],
        [H, -H],
        [0.92388, -0.38268],
    ];
    const G3: [[f32; 3]; 16] = [
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
        [1.0, -1.0, 0.0],
        [-1.0, -1.0, 0.0],
        [1.0, 0.0, 1.0],
        [-1.0, 0.0, 1.0],
        [1.0, 0.0, -1.0],
        [-1.0, 0.0, -1.0],
        [0.0, 1.0, 1.0],
        [0.0, -1.0, 1.0],
        [0.0, 1.0, -1.0],
        [0.0, -1.0, -1.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
        [0.0, -1.0, 1.0],
        [0.0, -1.0, -1.0],
    ];

    #[inline]
    pub fn hash(x: i32, y: i32, z: i32, seed: u32) -> u32 {
        let mut h = (x as u32).wrapping_mul(0x8da6_b343)
            ^ (y as u32).wrapping_mul(0xd816_3841)
            ^ (z as u32).wrapping_mul(0xcb1a_b31f)
            ^ seed.wrapping_mul(0x9e37_79b9);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2c1b_3c6d);
        h ^= h >> 12;
        h = h.wrapping_mul(0x297a_2d39);
        h ^ (h >> 15)
    }

    #[inline]
    pub fn wrap(i: i32, period: i32) -> i32 {
        if period > 0 { i.rem_euclid(period) } else { i }
    }

    /// 2D gradient noise with its derivatives: `[value, d/dx, d/dy]`, value in about
    /// `[-1, 1]`. `period > 0` makes it tile every `period` units.
    pub fn noised(x: f32, y: f32, period: i32, seed: u32) -> [f32; 3] {
        let (ix, iy) = (x.floor(), y.floor());
        let (fx, fy) = (x - ix, y - iy);
        let (x0, y0) = (ix as i32, iy as i32);
        let g =
            |a: i32, b: i32| G2[(hash(wrap(a, period), wrap(b, period), 0, seed) & 15) as usize];
        let ga = g(x0, y0);
        let gb = g(x0 + 1, y0);
        let gc = g(x0, y0 + 1);
        let gd = g(x0 + 1, y0 + 1);
        let va = ga[0] * fx + ga[1] * fy;
        let vb = gb[0] * (fx - 1.0) + gb[1] * fy;
        let vc = gc[0] * fx + gc[1] * (fy - 1.0);
        let vd = gd[0] * (fx - 1.0) + gd[1] * (fy - 1.0);
        let ux = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
        let uy = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
        let dux = 30.0 * fx * fx * (fx * (fx - 2.0) + 1.0);
        let duy = 30.0 * fy * fy * (fy * (fy - 2.0) + 1.0);
        let k = va - vb - vc + vd;
        let v = va + ux * (vb - va) + uy * (vc - va) + ux * uy * k;
        let dx = ga[0]
            + ux * (gb[0] - ga[0])
            + uy * (gc[0] - ga[0])
            + ux * uy * (ga[0] - gb[0] - gc[0] + gd[0])
            + dux * (uy * k + vb - va);
        let dy = ga[1]
            + ux * (gb[1] - ga[1])
            + uy * (gc[1] - ga[1])
            + ux * uy * (ga[1] - gb[1] - gc[1] + gd[1])
            + duy * (ux * k + vc - va);
        [v * 1.4, dx * 1.4, dy * 1.4]
    }

    #[inline]
    pub fn n2(x: f32, y: f32, seed: u32) -> f32 {
        noised(x, y, 0, seed)[0]
    }

    /// 3D gradient noise, about `[-1, 1]`.
    pub fn n3(x: f32, y: f32, z: f32, seed: u32) -> f32 {
        let (ix, iy, iz) = (x.floor(), y.floor(), z.floor());
        let (fx, fy, fz) = (x - ix, y - iy, z - iz);
        let (x0, y0, z0) = (ix as i32, iy as i32, iz as i32);
        let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
        let (u, v, w) = (fade(fx), fade(fy), fade(fz));
        let g = |dx: i32, dy: i32, dz: i32| {
            let gr = G3[(hash(x0 + dx, y0 + dy, z0 + dz, seed) & 15) as usize];
            gr[0] * (fx - dx as f32) + gr[1] * (fy - dy as f32) + gr[2] * (fz - dz as f32)
        };
        let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
        let x00 = l(g(0, 0, 0), g(1, 0, 0), u);
        let x10 = l(g(0, 1, 0), g(1, 1, 0), u);
        let x01 = l(g(0, 0, 1), g(1, 0, 1), u);
        let x11 = l(g(0, 1, 1), g(1, 1, 1), u);
        l(l(x00, x10, v), l(x01, x11, v), w) * 0.95
    }

    /// Fractal sum of `n2`, about `[-1, 1]`. Octaves are rotated against each other.
    pub fn fbm(x: f32, y: f32, octaves: u32, seed: u32) -> f32 {
        let (mut x, mut y) = (x, y);
        let (mut sum, mut amp) = (0.0, 0.5);
        for i in 0..octaves {
            sum += amp * n2(x, y, seed.wrapping_add(i * 7919));
            amp *= 0.5;
            (x, y) = (1.6 * x - 1.2 * y + 3.7, 1.2 * x + 1.6 * y - 1.3);
        }
        sum * 1.15
    }

    /// Like [`fbm`], but octaves with a wavelength (in input units) below `min_wl` fade out.
    pub fn fbm_lod(x: f32, y: f32, octaves: u32, min_wl: f32, seed: u32) -> f32 {
        let (mut x, mut y) = (x, y);
        let (mut sum, mut amp, mut wl) = (0.0, 0.5, 1.0);
        for i in 0..octaves {
            let w = super::sstep(1.0, 2.0, wl / min_wl.max(1e-6));
            if w <= 0.0 {
                break;
            }
            sum += amp * w * n2(x, y, seed.wrapping_add(i * 7919));
            amp *= 0.5;
            wl *= 0.5;
            (x, y) = (1.6 * x - 1.2 * y + 3.7, 1.2 * x + 1.6 * y - 1.3);
        }
        sum * 1.15
    }

    /// Tileable fractal sum: tiles every `period` units (frequencies double, so it stays
    /// tileable).
    pub fn fbm_tile(x: f32, y: f32, octaves: u32, period: i32, seed: u32) -> f32 {
        let (mut sum, mut amp, mut f) = (0.0, 0.5, 1.0);
        for i in 0..octaves {
            sum += amp * noised(x * f, y * f, period * f as i32, seed.wrapping_add(i * 7919))[0];
            amp *= 0.5;
            f *= 2.0;
        }
        sum * 1.15
    }

    /// Ridged multifractal, about `[0, 1]`, with the same octave fade as [`fbm_lod`].
    pub fn ridged_lod(x: f32, y: f32, octaves: u32, min_wl: f32, seed: u32) -> f32 {
        let (mut x, mut y) = (x, y);
        let (mut sum, mut amp, mut wl, mut weight, mut norm) = (0.0, 1.0, 1.0, 1.0f32, 0.0);
        for i in 0..octaves {
            let w = super::sstep(1.0, 2.0, wl / min_wl.max(1e-6));
            let n = 1.0 - n2(x, y, seed.wrapping_add(i * 977)).abs();
            let n = n * n * weight;
            weight = (n * 1.8).clamp(0.0, 1.0);
            sum += n * amp * w;
            norm += amp;
            amp *= 0.5;
            wl *= 0.5;
            (x, y) = (1.6 * x - 1.2 * y + 3.7, 1.2 * x + 1.6 * y - 1.3);
        }
        sum / norm
    }

    /// Erosion-like fractal sum: derivatives of the larger octaves damp the smaller ones, so
    /// slopes get gullies and flats stay smooth. About `[-1, 1]`.
    pub fn eroded(x: f32, y: f32, octaves: u32, min_wl: f32, seed: u32) -> f32 {
        let (mut x, mut y) = (x, y);
        let (mut sum, mut amp, mut wl) = (0.0, 0.5, 1.0);
        let (mut dx, mut dy) = (0.0, 0.0);
        for i in 0..octaves {
            let w = super::sstep(1.0, 2.0, wl / min_wl.max(1e-6));
            if w <= 0.0 {
                break;
            }
            let n = noised(x, y, 0, seed.wrapping_add(i * 131));
            dx += n[1] * amp;
            dy += n[2] * amp;
            sum += amp * w * n[0] / (1.0 + 2.0 * (dx * dx + dy * dy));
            amp *= 0.5;
            wl *= 0.5;
            (x, y) = (1.6 * x - 1.2 * y + 3.7, 1.2 * x + 1.6 * y - 1.3);
        }
        sum * 1.3
    }

    pub fn fbm3(x: f32, y: f32, z: f32, octaves: u32, seed: u32) -> f32 {
        let (mut sum, mut amp, mut f) = (0.0, 0.5, 1.0);
        for i in 0..octaves {
            sum += amp
                * n3(
                    x * f + i as f32 * 3.1,
                    y * f,
                    z * f,
                    seed.wrapping_add(i * 31),
                );
            amp *= 0.5;
            f *= 2.03;
        }
        sum * 1.15
    }

    /// Tileable cellular noise: distances to the nearest and second nearest feature point
    /// (in cell units) and the nearest cell's hash.
    pub fn cells(x: f32, y: f32, period: i32, seed: u32) -> (f32, f32, u32) {
        let (ix, iy) = (x.floor() as i32, y.floor() as i32);
        let (mut f1, mut f2, mut id) = (9.0f32, 9.0f32, 0u32);
        for oy in -1..=1 {
            for ox in -1..=1 {
                let (cx, cy) = (ix + ox, iy + oy);
                let h = hash(wrap(cx, period), wrap(cy, period), 7, seed);
                let px = cx as f32 + (h & 0xffff) as f32 / 65536.0;
                let py = cy as f32 + (h >> 16) as f32 / 65536.0;
                let d = ((px - x).powi(2) + (py - y).powi(2)).sqrt();
                if d < f1 {
                    f2 = f1;
                    f1 = d;
                    id = h;
                } else if d < f2 {
                    f2 = d;
                }
            }
        }
        (f1, f2, id)
    }
}

// ============================================================================ textures

/// A texture set: albedo (sRGB), tangent-space normal and ORM (R occlusion, G roughness,
/// B metal), all the same size.
struct TexSet {
    albedo: Handle<Image>,
    normal: Handle<Image>,
    orm: Handle<Image>,
}

/// One texel of a material: linear albedo, height (m), roughness, metal, occlusion, alpha.
#[derive(Clone, Copy)]
struct Texel {
    albedo: Vec3,
    height: f32,
    rough: f32,
    metal: f32,
    ao: f32,
    alpha: f32,
}

impl Default for Texel {
    fn default() -> Self {
        Self {
            albedo: Vec3::splat(0.5),
            height: 0.0,
            rough: 0.8,
            metal: 0.0,
            ao: 1.0,
            alpha: 1.0,
        }
    }
}

fn linear_to_srgb8(v: f32) -> u8 {
    let v = sat(v);
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5) as u8
}

fn unorm8(v: f32) -> u8 {
    (sat(v) * 255.0 + 0.5) as u8
}

/// Build a texture set `w`×`h` texels covering `size_m` metres horizontally; `f(u, v)` gets
/// texel coordinates in `[0, 1)`. Normals come from the height (metres), wrapping at the
/// edges when `tile` is set.
fn bake_set(
    world: &mut World,
    w: u32,
    h: u32,
    size_m: f32,
    tile: bool,
    normal_strength: f32,
    f: impl Fn(f32, f32) -> Texel + Sync,
) -> TexSet {
    let texels: Vec<Vec<Texel>> = par_map(h as usize, |y| {
        (0..w)
            .map(|x| f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32))
            .collect()
    });
    let at = |x: i32, y: i32| -> f32 {
        let (x, y) = if tile {
            (x.rem_euclid(w as i32), y.rem_euclid(h as i32))
        } else {
            (x.clamp(0, w as i32 - 1), y.clamp(0, h as i32 - 1))
        };
        texels[y as usize][x as usize].height
    };
    let texel_m = size_m / w as f32;
    let mut albedo = Vec::with_capacity((w * h * 4) as usize);
    let mut normal = Vec::with_capacity((w * h * 4) as usize);
    let mut orm = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let t = texels[y as usize][x as usize];
            albedo.extend_from_slice(&[
                linear_to_srgb8(t.albedo.x),
                linear_to_srgb8(t.albedo.y),
                linear_to_srgb8(t.albedo.z),
                unorm8(t.alpha),
            ]);
            let du = (at(x + 1, y) - at(x - 1, y)) / (2.0 * texel_m) * normal_strength;
            let dv = (at(x, y + 1) - at(x, y - 1)) / (2.0 * texel_m) * normal_strength;
            let n = Vec3::new(-du, dv, 1.0).normalize();
            normal.extend_from_slice(&[
                unorm8(n.x * 0.5 + 0.5),
                unorm8(n.y * 0.5 + 0.5),
                unorm8(n.z * 0.5 + 0.5),
                255,
            ]);
            orm.extend_from_slice(&[unorm8(t.ao), unorm8(t.rough), unorm8(t.metal), 255]);
        }
    }
    TexSet {
        albedo: add_image(world, Image::new(w, h, albedo, true)),
        normal: add_image(world, Image::new(w, h, normal, false)),
        orm: add_image(world, Image::new(w, h, orm, false)),
    }
}

struct Textures {
    ground_near: TexSet,
    ground_mid: TexSet,
    ground_far: TexSet,
    dirt: TexSet,
    rock: TexSet,
    wood: TexSet,
    canvas: TexSet,
    metal: TexSet,
    lattice: TexSet,
    moon: TexSet,
}

/// Mean of the ground albedo textures; terrain vertex colours are divided by it.
const GROUND_TEX_MEAN: f32 = 0.7;

impl Textures {
    fn generate(world: &mut World) -> Self {
        Self {
            ground_near: bake_set(world, 1024, 1024, 2.5, true, 1.0, ground_near_texel),
            ground_mid: bake_set(world, 512, 512, 24.0, true, 1.0, ground_mid_texel),
            ground_far: bake_set(world, 512, 512, 512.0, true, 1.0, ground_far_texel),
            dirt: bake_set(world, 1024, 1024, 2.5, true, 1.0, dirt_texel),
            rock: bake_set(world, 1024, 1024, 1.6, true, 1.6, rock_texel),
            wood: bake_set(world, 512, 1024, 0.5, true, 1.0, wood_texel),
            canvas: bake_set(world, 1024, 1024, 3.4, false, 1.0, canvas_texel),
            metal: bake_set(world, 512, 512, 2.0, true, 1.0, metal_texel),
            lattice: bake_set(world, 128, 512, 1.0, true, 0.0, lattice_texel),
            moon: bake_set(world, 1024, 512, 1.0, true, 0.0, moon_texel),
        }
    }
}

/// Short dry turf seen from above: four layers of thin blades over dark soil (2.5 m tile).
/// A modulator around [`GROUND_TEX_MEAN`]: the terrain's vertex colour carries the hue.
fn ground_near_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let soil = fbm_tile(u * 10.0, v * 10.0, 4, 10, 201);
    let mut albedo = Vec3::new(0.36, 0.31, 0.27) * (0.85 + 0.25 * soil);
    let mut height = soil * 0.002;
    let mut ao = 0.55f32;
    let mut rough = 0.95;
    let density = sstep(-0.6, 0.25, fbm_tile(u * 5.0, v * 5.0, 3, 5, 202));
    let green_zone = sstep(0.0, 0.5, fbm_tile(u * 3.0 + 0.5, v * 3.0, 3, 3, 204));
    for layer in 0..4u32 {
        let n = [30.0f32, 40.0, 52.0, 66.0][layer as usize];
        let period = n as i32;
        let (x, y) = (u * n, v * n);
        let (ix, iy) = (x.floor() as i32, y.floor() as i32);
        for oy in -1..=1 {
            for ox in -1..=1 {
                let (cx, cy) = (ix + ox, iy + oy);
                for strand in 0..2u32 {
                    let h = hash(
                        wrap(cx, period),
                        wrap(cy, period),
                        (layer * 2 + strand) as i32,
                        203,
                    );
                    if (h & 255) as f32 / 255.0 > 0.3 + 0.65 * density {
                        continue;
                    }
                    let sx = cx as f32 + ((h >> 8) & 255) as f32 / 256.0;
                    let sy = cy as f32 + ((h >> 16) & 255) as f32 / 256.0;
                    let ang = ((h >> 24) & 255) as f32 / 256.0 * TAU;
                    let len = 0.55 + 0.4 * ((h >> 4) & 15) as f32 / 15.0;
                    let (dx, dy) = (ang.cos(), ang.sin());
                    let (px, py) = (x - sx, y - sy);
                    let t = (px * dx + py * dy) / len;
                    if !(0.0..=1.0).contains(&t) {
                        continue;
                    }
                    let d = (-px * dy + py * dx).abs();
                    let w = 0.075 * (1.0 - 0.75 * t);
                    if d >= w {
                        continue;
                    }
                    let hgt = 0.004 + layer as f32 * 0.003 + t * 0.003 + (1.0 - d / w) * 0.0008;
                    if hgt > height {
                        height = hgt;
                        let green = (((h >> 2) & 3) == 0) as i32 as f32 * green_zone;
                        let tone = mix3(
                            Vec3::new(1.0, 0.92, 0.74),
                            Vec3::new(0.72, 0.84, 0.5),
                            green,
                        );
                        let lift = 0.62 + 0.11 * layer as f32 + 0.15 * t;
                        albedo = tone * lift * (0.9 + 0.1 * (1.0 - d / w));
                        ao = 0.62 + 0.1 * layer as f32 + 0.15 * t;
                        rough = 0.78;
                    }
                }
            }
        }
    }
    Texel {
        albedo: albedo.min(Vec3::ONE),
        height,
        rough,
        ao: ao.min(1.0),
        ..Default::default()
    }
}

/// Trodden soil with grit and a little fine gravel (2.5 m tile). Alpha is ragged noise: the
/// path decal cuts its edges with it.
fn dirt_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let soil = fbm_tile(u * 6.0, v * 6.0, 6, 6, 211);
    let clods = fbm_tile(u * 40.0, v * 40.0, 3, 40, 217);
    let grit = noised(u * 300.0, v * 300.0, 300, 212)[0];
    let mut albedo = Vec3::new(0.8, 0.72, 0.62)
        * (0.86 + 0.2 * soil)
        * (0.92 + 0.1 * clods)
        * (0.93 + 0.12 * grit);
    let mut height = soil * 0.006 + clods * 0.0015 + grit * 0.0003;
    let mut rough = 0.92;
    let mut ao: f32 = 1.0;
    let gravel_zone = sstep(-0.2, 0.4, fbm_tile(u * 5.0 + 3.0, v * 5.0, 3, 5, 218));
    for (n, density, lift, seed) in [
        (150.0f32, 0.55f32, 0.0025f32, 213u32),
        (60.0, 0.12, 0.005, 214),
    ] {
        let wx = fbm_tile(u * n * 0.25, v * n * 0.25, 2, (n * 0.25) as i32, seed + 10) * 0.45;
        let (f1, f2, id) = cells(u * n + wx, v * n - wx, n as i32, seed);
        let r = 0.18 + 0.25 * ((id >> 8) & 255) as f32 / 255.0;
        let has = (((id >> 16) & 255) as f32 / 255.0) < density * gravel_zone;
        if has && f1 < r && f2 - f1 > 0.05 {
            let dome = (1.0 - (f1 / r).powi(2)).max(0.0).sqrt();
            let tone = 0.7 + 0.35 * (id & 255) as f32 / 255.0;
            let warm = ((id >> 3) & 1) as f32;
            let stone = mix3(
                Vec3::new(0.78, 0.76, 0.74),
                Vec3::new(0.86, 0.74, 0.6),
                warm,
            ) * tone;
            let k = sstep(0.0, 0.5, dome);
            albedo = mix3(albedo * 0.8, stone, k * 0.8);
            height = height.max(dome * lift);
            rough = 0.92 - 0.15 * k;
        } else if has && f1 < r * 1.4 {
            ao *= 0.7 + 0.3 * sstep(r, r * 1.4, f1);
        }
    }
    let scuff = sstep(0.2, 0.5, fbm_tile(u * 14.0, v * 5.0, 3, 14, 215));
    albedo *= 1.0 - 0.08 * scuff;
    height -= scuff * 0.002;
    let alpha = 0.5 + 0.5 * sat(0.5 + 0.9 * fbm_tile(u * 12.0, v * 12.0, 5, 12, 216));
    Texel {
        albedo,
        height,
        rough,
        ao,
        alpha,
        ..Default::default()
    }
}

/// Grassland seen from tens of metres: tussock clumps, bare patches and stones (24 m tile).
fn ground_mid_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let (f1, _, id) = cells(u * 26.0, v * 26.0, 26, 41);
    let r = 0.32 + 0.18 * ((id >> 8) & 255) as f32 / 255.0;
    let clump = sstep(r, r * 0.4, f1);
    let patch = fbm_tile(u * 6.0, v * 6.0, 5, 6, 42);
    let fine = fbm_tile(u * 64.0, v * 64.0, 3, 64, 43);
    let mut albedo = Vec3::new(0.62, 0.6, 0.56) * (0.9 + 0.2 * fine);
    albedo = mix3(
        albedo,
        Vec3::new(0.95, 0.85, 0.6),
        clump * (0.6 + 0.3 * patch),
    );
    let shade = 0.72 + 0.28 * clump;
    let (s1, _, sid) = cells(u * 90.0, v * 90.0, 90, 44);
    let stone = if (sid & 255) < 22 && s1 < 0.3 {
        1.0 - s1 / 0.3
    } else {
        0.0
    };
    albedo = mix3(albedo * shade, Vec3::splat(0.82), stone * 0.8);
    Texel {
        albedo,
        height: clump * 0.12 + fine * 0.01 + stone * 0.04,
        rough: 0.92,
        ao: 0.75 + 0.25 * clump,
        ..Default::default()
    }
}

/// Mountainsides seen from kilometres: gullies, strata and scree fans (512 m tile).
fn ground_far_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let ridges = {
        let mut s = 0.0;
        let mut amp = 0.5;
        let mut f = 4.0;
        for i in 0..6 {
            let n = noised(u * f, v * f, f as i32, 51 + i)[0];
            s += amp * (1.0 - n.abs()).powi(2);
            amp *= 0.5;
            f *= 2.0;
        }
        s
    };
    let broad = fbm_tile(u * 3.0, v * 3.0, 4, 3, 52);
    let strata = (v * 140.0 + broad * 9.0).sin() * 0.5 + 0.5;
    let albedo = Vec3::splat(0.62 + 0.25 * ridges + 0.08 * broad) * (0.92 + 0.12 * strata);
    Texel {
        albedo: albedo.min(Vec3::ONE),
        height: ridges * 6.0 + broad * 4.0,
        rough: 0.9,
        ao: 0.7 + 0.3 * ridges,
        ..Default::default()
    }
}

/// Weathered stone: grain, sparse thin cracks, mineral speckle and lichen (1.6 m tile).
fn rock_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let grain = fbm_tile(u * 5.0, v * 5.0, 8, 5, 61);
    let broad = fbm_tile(u * 2.0, v * 2.0, 3, 2, 67);
    let speck = noised(u * 420.0, v * 420.0, 420, 62)[0];
    // Cracks along the zero lines of warped noise, only in places.
    let w = fbm_tile(u * 4.0, v * 4.0, 3, 4, 63) * 0.5;
    let c1 = noised(u * 3.0 + w, v * 3.0 - w, 3, 64)[0].abs();
    let c2 = noised(u * 9.0 - w, v * 9.0 + w, 9, 68)[0].abs();
    let where1 = sstep(0.0, 0.4, fbm_tile(u * 2.0 + 0.7, v * 2.0, 3, 2, 70));
    let crack =
        sat(sstep(0.016, 0.0, c1) * where1 + 0.5 * sstep(0.012, 0.0, c2) * sstep(0.1, 0.5, grain));
    let pits = sstep(0.6, 0.8, noised(u * 90.0, v * 90.0, 90, 69)[0]);
    let base =
        Vec3::new(0.34, 0.27, 0.2) * (0.72 + 0.4 * grain + 0.2 * broad) * (0.93 + 0.1 * speck);
    // Grit: light and dark mineral grains.
    let grit = noised(u * 160.0, v * 160.0, 160, 71)[0];
    let (g1, _, gid) = cells(u * 220.0, v * 220.0, 220, 72);
    let spark = sstep(0.3, 0.15, g1) * ((gid & 7) == 0) as i32 as f32;
    let mut albedo =
        base * (1.0 - 0.45 * crack - 0.2 * pits) * (0.9 + 0.18 * grit) * (1.0 + 0.5 * spark);
    // Lichen: small round colonies, clustered.
    let cluster = sstep(0.05, 0.4, fbm_tile(u * 3.0 + 4.0, v * 3.0, 4, 3, 65));
    let (f1, _, id) = cells(u * 40.0, v * 40.0, 40, 66);
    let r = 0.15 + 0.35 * ((id >> 8) & 255) as f32 / 255.0;
    let colony = sstep(r, r * 0.6, f1) * cluster * ((id & 3) != 0) as i32 as f32;
    let lichen_col = match (id >> 4) & 3 {
        0 => Vec3::new(0.5, 0.3, 0.1),
        1 => Vec3::new(0.08, 0.08, 0.07),
        _ => Vec3::new(0.4, 0.4, 0.3),
    };
    albedo = mix3(
        albedo,
        lichen_col * (0.85 + 0.25 * speck),
        colony * (1.0 - crack) * 0.6,
    );
    Texel {
        albedo,
        height: grain * 0.006 + broad * 0.004 - crack * 0.002 - pits * 0.0006
            + colony * 0.0004
            + grit * 0.0005,
        rough: 0.86 + 0.08 * colony - 0.05 * speck.max(0.0),
        ao: 1.0 - 0.4 * crack - 0.2 * pits,
        ..Default::default()
    }
}

/// Sun-bleached planks: grain along V, checks and knots (0.5 m wide, 1 m long tile).
fn wood_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    // Grain: rings stretched along v, warped.
    let warp = fbm_tile(u * 4.0, v * 2.0, 4, 2, 71) * 0.6;
    let knot = {
        let (f1, _, id) = cells(u * 3.0, v * 1.5, 3, 72);
        if (id & 3) == 0 {
            sstep(0.25, 0.0, f1)
        } else {
            0.0
        }
    };
    let rings = ((u * 60.0 + warp * 6.0 + knot * 4.0) * TAU).sin() * 0.5 + 0.5;
    let fibre = noised(u * 300.0, v * 6.0, 0, 73)[0];
    let check = (1.0 - noised(u * 90.0, v * 3.0, 0, 74)[0].abs() * 18.0).max(0.0)
        * sstep(0.1, 0.4, fbm_tile(u * 2.0, v * 4.0, 3, 2, 75));
    let tone = 0.85 + 0.15 * fbm_tile(u * 2.0, v * 1.0, 3, 1, 76);
    let mut albedo =
        Vec3::new(0.3, 0.215, 0.15) * tone * (0.82 + 0.18 * rings) * (0.95 + 0.1 * fibre);
    albedo *= 1.0 - 0.6 * check - 0.3 * knot;
    Texel {
        albedo,
        height: rings * 0.0006 + fibre * 0.0002 - check * 0.0015 - knot * 0.0008,
        rough: 0.82 - 0.05 * rings,
        ao: 1.0 - 0.5 * check,
        ..Default::default()
    }
}

/// The awning: striped canvas with weave, grime and a scalloped valance strip (not tiled:
/// v in 0..0.82 is the roof, the rest the valance).
fn canvas_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let stripe = ((u * 9.0).fract() < 0.5) as i32 as f32;
    let cream = Vec3::new(0.62, 0.52, 0.37);
    let madder = Vec3::new(0.40, 0.10, 0.05);
    let mut albedo = mix3(cream, madder, stripe);
    // Weave.
    let (wu, wv) = (u * 900.0, v * 900.0);
    let weave = (wu * TAU).sin() * (wv * TAU).sin();
    let fuzz = noised(u * 220.0, v * 220.0, 0, 81)[0];
    // Grime: darker toward the edges and in streaks running down the slope (v).
    let edge = 1.0 - sstep(0.0, 0.08, u.min(1.0 - u)) * sstep(0.0, 0.1, v.min(0.82 - v).abs());
    let streak = sat(fbm(u * 30.0, v * 2.0, 4, 82) * 1.5);
    let stain = sat(fbm(u * 5.0, v * 5.0, 5, 83));
    albedo *= (1.0 - 0.3 * edge - 0.22 * streak - 0.25 * stain) * (0.95 + 0.05 * fuzz);
    // Sun fading: the reds are paler.
    albedo = mix3(albedo, Vec3::splat(albedo.dot(Vec3::splat(0.333))), 0.15);
    let mut alpha = 1.0;
    if v > 0.82 {
        // Valance: scalloped bottom edge.
        let s = (u * 18.0).fract() - 0.5;
        let bottom = 0.99 - 0.06 * (1.0 - 4.0 * s * s);
        alpha = if v < bottom { 1.0 } else { 0.0 };
        albedo *= 0.92;
    }
    Texel {
        albedo,
        height: weave * 0.00012 + fuzz * 0.00005,
        rough: 0.9,
        ao: 1.0 - 0.15 * edge,
        alpha,
        ..Default::default()
    }
}

/// Painted metal: chipped paint over steel, rust streaks running down (2 m tile).
fn metal_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let chips = fbm_tile(u * 12.0, v * 12.0, 6, 12, 91);
    let chip = sstep(0.32, 0.36, chips);
    let streak = sat(fbm_tile(u * 40.0, v * 3.0, 4, 3, 92) * 2.0 - 0.2);
    let grime = fbm_tile(u * 4.0, v * 4.0, 4, 4, 93);
    let scratches = (1.0 - noised(u * 6.0, v * 160.0, 0, 94)[0].abs() * 30.0).max(0.0);
    let paint = Vec3::splat(0.85) * (0.92 + 0.08 * grime);
    let rust = Vec3::new(0.32, 0.14, 0.06) * (0.8 + 0.4 * grime);
    let steel = Vec3::new(0.55, 0.55, 0.56);
    let mut albedo = mix3(paint, rust, streak * 0.55);
    albedo = mix3(albedo, steel, chip);
    albedo = mix3(albedo, steel * 1.1, scratches * 0.4 * (1.0 - chip));
    Texel {
        albedo,
        height: -chip * 0.0002 + grime * 0.00008,
        rough: 0.5 + 0.3 * streak - 0.15 * chip + 0.1 * grime,
        metal: chip.max(scratches * 0.5),
        ..Default::default()
    }
}

/// Lattice panel for the relay tower: legs at the sides, diagonals and rungs (alpha).
fn lattice_texel(u: f32, v: f32) -> Texel {
    let x = u;
    let y = (v * 4.0).fract();
    let bar = |d: f32, w: f32| (d.abs() < w) as i32 as f32;
    let a = bar(x - 0.03, 0.04) + bar(x - 0.97, 0.04);
    let diag1 = bar(x - y, 0.025);
    let diag2 = bar(x - (1.0 - y), 0.025);
    let rung = bar(y - 0.02, 0.025);
    let alpha = sat(a + diag1 + diag2 + rung);
    Texel {
        albedo: Vec3::new(0.32, 0.31, 0.3),
        rough: 0.5,
        metal: 0.6,
        alpha,
        ..Default::default()
    }
}

/// Moon surface: equirectangular maria and craters.
fn moon_texel(u: f32, v: f32) -> Texel {
    use noise::*;
    let lat = (v - 0.5) * PI;
    let lon = u * TAU;
    let d = Vec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
    let maria = sstep(0.05, 0.25, fbm3(d.x * 1.6, d.y * 1.6, d.z * 1.6, 5, 101));
    let mut crater = 0.0;
    for (scale, seed) in [(6.0f32, 102u32), (14.0, 103), (30.0, 104)] {
        let p = d * scale;
        let c = n3(p.x, p.y, p.z, seed);
        crater += sstep(0.55, 0.62, c) * 0.3 - sstep(0.45, 0.55, c) * sstep(0.62, 0.55, c) * 0.1;
    }
    let tone =
        0.42 - 0.2 * maria + crater + 0.05 * fbm3(d.x * 20.0, d.y * 20.0, d.z * 20.0, 3, 105);
    Texel {
        albedo: Vec3::new(0.95, 0.93, 0.9) * tone.clamp(0.08, 0.8),
        rough: 1.0,
        ao: 0.0,
        ..Default::default()
    }
}

struct Materials {
    ground_near: Handle<StandardMaterial>,
    ground_mid: Handle<StandardMaterial>,
    ground_far: Handle<StandardMaterial>,
    dirt: Handle<StandardMaterial>,
    rock: Handle<StandardMaterial>,
    wood: Handle<StandardMaterial>,
    canvas: Handle<StandardMaterial>,
    metal: Handle<StandardMaterial>,
    iron: Handle<StandardMaterial>,
    grass: Handle<StandardMaterial>,
    lattice: Handle<StandardMaterial>,
    crystal: Handle<StandardMaterial>,
    pottery: Handle<StandardMaterial>,
    burlap: Handle<StandardMaterial>,
    lamp_glass: Handle<StandardMaterial>,
    beacon_red: Handle<StandardMaterial>,
    beacon_green: Handle<StandardMaterial>,
    window: Handle<StandardMaterial>,
}

fn textured(set: &TexSet, uv_scale: f32) -> StandardMaterial {
    StandardMaterial {
        base_color: [1.0; 4],
        perceptual_roughness: 1.0,
        metallic: 1.0,
        base_color_texture: Some(set.albedo),
        normal_texture: Some(set.normal),
        metallic_roughness_texture: Some(set.orm),
        occlusion_texture: Some(set.orm),
        uv_scale: [uv_scale, uv_scale],
        ..Default::default()
    }
}

impl Materials {
    fn new(world: &mut World, t: &Textures) -> Self {
        let ground = |set: &TexSet, size: f32| StandardMaterial {
            reflectance: 0.4,
            ..textured(set, 1.0 / size)
        };
        let lamp = |c: [f32; 3], s: f32| StandardMaterial {
            base_color: [c[0], c[1], c[2], 1.0],
            emissive: c,
            emissive_strength: s,
            perceptual_roughness: 0.2,
            ..Default::default()
        };
        Self {
            ground_near: add_mat(world, ground(&t.ground_near, 2.5)),
            ground_mid: add_mat(world, ground(&t.ground_mid, 24.0)),
            ground_far: add_mat(world, ground(&t.ground_far, 512.0)),
            dirt: add_mat(
                world,
                StandardMaterial {
                    alpha_mode: AlphaMode::Mask(0.5),
                    ..ground(&t.dirt, 2.5)
                },
            ),
            rock: add_mat(world, textured(&t.rock, 1.0 / 1.6)),
            wood: add_mat(world, textured(&t.wood, 1.0)),
            canvas: add_mat(
                world,
                StandardMaterial {
                    double_sided: true,
                    subsurface: 0.55,
                    alpha_mode: AlphaMode::Mask(0.5),
                    ..textured(&t.canvas, 1.0)
                },
            ),
            metal: add_mat(world, textured(&t.metal, 0.5)),
            iron: add_mat(
                world,
                StandardMaterial::color(0.09, 0.085, 0.08)
                    .with_metallic(0.85)
                    .with_roughness(0.55),
            ),
            grass: add_mat(
                world,
                StandardMaterial {
                    base_color: [1.0; 4],
                    perceptual_roughness: 0.85,
                    reflectance: 0.25,
                    subsurface: 0.45,
                    ..Default::default()
                },
            ),
            lattice: add_mat(
                world,
                StandardMaterial {
                    double_sided: true,
                    alpha_mode: AlphaMode::Blend,
                    ..textured(&t.lattice, 1.0)
                },
            ),
            crystal: add_mat(
                world,
                StandardMaterial {
                    base_color: [0.25, 0.75, 0.8, 1.0],
                    perceptual_roughness: 0.12,
                    reflectance: 0.7,
                    subsurface: 0.6,
                    emissive: [0.2, 0.85, 0.9],
                    emissive_strength: 25.0,
                    ..Default::default()
                },
            ),
            pottery: add_mat(
                world,
                StandardMaterial::color(1.0, 1.0, 1.0).with_roughness(0.42),
            ),
            // The canvas weave without its stripes.
            burlap: add_mat(
                world,
                StandardMaterial {
                    base_color: [0.3, 0.22, 0.14, 1.0],
                    base_color_texture: None,
                    subsurface: 0.2,
                    ..textured(&t.canvas, 3.0)
                },
            ),
            lamp_glass: add_mat(world, lamp([1.0, 0.62, 0.3], 900.0)),
            beacon_red: add_mat(world, lamp([1.0, 0.06, 0.03], 60_000.0)),
            beacon_green: add_mat(world, lamp([0.2, 1.0, 0.35], 4_000.0)),
            window: add_mat(world, lamp([1.0, 0.7, 0.4], 900.0)),
        }
    }
}

// ============================================================================ terrain

/// Centre line of the footpath (x, z), from behind the player to the plateau edge.
const PATH: [Vec2; 9] = [
    Vec2::new(-0.8, 9.0),
    Vec2::new(-0.2, 3.0),
    Vec2::new(0.6, -2.0),
    Vec2::new(2.2, -7.0),
    Vec2::new(2.0, -13.0),
    Vec2::new(-0.5, -21.0),
    Vec2::new(-4.5, -31.0),
    Vec2::new(-9.0, -42.0),
    Vec2::new(-14.0, -56.0),
];

fn path_distance(p: Vec2) -> f32 {
    let mut best = f32::MAX;
    for w in PATH.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        let t = sat((p - a).dot(ab) / ab.length_squared());
        best = best.min((a + ab * t).distance(p));
    }
    // Wobble the edges.
    best + noise::n2(p.x * 0.6, p.y * 0.6, 5) * 0.25
}

/// How flat the ground is kept around the stall, the hab and the path (0..1).
fn calm(p: Vec2) -> f32 {
    let stall = sstep(6.0, 3.0, p.distance(STALL));
    let hab = sstep(7.0, 4.0, p.distance(HAB));
    stall.max(hab).max(sstep(2.5, 0.8, path_distance(p)) * 0.6)
}

/// Distance from the forward edge of the plateau (positive beyond it), metres.
fn past_edge(x: f32, z: f32) -> f32 {
    let f = 4.0 - z;
    let edge = 60.0
        + 0.0012 * x * x / (1.0 + (x / 600.0).powi(2))
        + 14.0 * noise::fbm(x / 70.0, 1.3, 3, 21);
    f - edge
}

/// The spur that carries the lookout: (height, how much of the spur's top this is (0..1)).
fn spur(p: Vec2, min_wl: f32) -> (f32, f32) {
    let a = Vec2::new(26.0, -26.0);
    let b = LOOKOUT;
    let ab = b - a;
    let s = (p - a).dot(ab) / ab.length_squared();
    let sc = s.clamp(0.0, 1.0);
    let d = (a + ab * sc).distance(p);
    let width =
        24.0 - 10.0 * sc + 6.0 * noise::fbm_lod(p.x / 40.0, p.y / 40.0, 3, min_wl / 40.0, 22);
    let top = -2.8 * sc + 0.6 * noise::fbm_lod(p.x / 9.0, p.y / 9.0, 3, min_wl / 9.0, 23);
    let beyond = (s - 1.0).max(0.0) * ab.length();
    let off = (d - width).max(0.0) + beyond;
    let knoll = 4.6 * (-(p.distance(b) / 16.0).powi(2)).exp();
    let h = top + knoll - 128.0 * (1.0 - (-off / 55.0).exp());
    (h, sstep(14.0, 0.0, off))
}

/// Ground height (m) at (x, z), leaving out detail with wavelengths below `min_wl` metres.
fn height(x: f32, z: f32, min_wl: f32) -> f32 {
    use noise::*;
    let p = Vec2::new(x, z);
    let f = 4.0 - z;
    let t = past_edge(x, z);

    // Plateau: gentle swells, kept calm around the stall and the path.
    let swell = fbm_lod(x / 40.0, z / 40.0, 4, min_wl / 40.0, 1) * 0.9
        + fbm_lod(x / 9.0, z / 9.0, 4, min_wl / 9.0, 2) * 0.25
        + fbm_lod(x / 1.6, z / 1.6, 3, min_wl / 1.6, 3) * 0.035;
    let calmness = calm(p);
    let mut plateau = swell * (1.0 - 0.8 * calmness) - 0.012 * f.max(0.0);
    // Worn path: a shallow trough.
    plateau -= 0.07 * sstep(1.2, 0.3, path_distance(p));
    // A lip of bedrock at the edge.
    plateau += 1.4 * (-(t / 7.0).powi(2)).exp();

    // Escarpment and valley, then the relay ridge.
    let drop = if t > 0.0 {
        -128.0 * (1.0 - (-t / 62.0).exp())
    } else {
        0.0
    };
    let crest = 905.0 + 45.0 * fbm(x / 400.0, 7.7, 3, 24);
    let rise = (146.0 + 22.0 * fbm(x / 260.0, 3.3, 4, 25)) * sstep(crest - 330.0, crest, f);
    let dip = -40.0 * sstep(crest + 60.0, crest + 420.0, f);
    let slopes = eroded(x / 230.0, z / 230.0, 9, min_wl / 230.0, 4)
        * 26.0
        * sstep(0.0, 120.0, t)
        * (1.0 - 0.6 * sstep(crest - 150.0, crest, f) * sstep(crest + 100.0, crest, f));
    let mut h = plateau + drop + rise + dip + slopes;

    // Beyond: foothills, a rocky middle range and a far snowy massif, each farther one taller
    // so their silhouettes stack. A low pass opens where the sun sets.
    let az = (x - EYE.x as f32).atan2(EYE.z as f32 - z);
    let pass = 1.0 - 0.75 * (-((az - SUN_AZIMUTH_DEG.to_radians()) / 0.3).powi(2)).exp();
    let warp = Vec2::new(
        fbm(x / 7000.0, z / 7000.0, 3, 5),
        fbm(x / 7000.0 + 9.0, z / 7000.0, 3, 6),
    ) * 2200.0;
    let q = p + warp;
    let foot = eroded(q.x / 1500.0, q.y / 1500.0, 10, min_wl / 1500.0, 7) * 0.5 + 0.5;
    h += sstep(crest + 400.0, 3200.0, f) * (40.0 + 300.0 * foot * foot) * pass;
    let bell = |c: f32, w: f32| (-((f - c) / w).powi(2)).exp();
    let ridges = |scale: f32, seed: u32| {
        let r = ridged_lod(q.x / scale, q.y / scale, 11, min_wl / scale, seed);
        let mass = fbm_lod(
            q.x / (scale * 2.3),
            q.y / (scale * 2.3),
            4,
            min_wl / (scale * 2.3),
            seed + 50,
        ) * 0.5
            + 0.5;
        (0.2 + 0.8 * r.powf(1.35)) * (0.55 + 0.7 * mass)
    };
    h += bell(9_500.0, 3_200.0) * 1_450.0 * ridges(4_200.0, 8) * pass;
    h += sstep(14_000.0, 21_000.0, f) * 3_700.0 * ridges(7_500.0, 9) * pass;
    h += eroded(x / 800.0, z / 800.0, 9, min_wl / 800.0, 10) * 70.0 * sstep(1_800.0, 6_000.0, f);

    // The spur with the lookout.
    let (spur_h, _) = spur(p, min_wl);
    h = smax(h, spur_h, 6.0);
    h
}

/// How much the ground is worn bare by feet (0..1).
fn path_wear(p: Vec2) -> f32 {
    let d = path_distance(p);
    let around_stall = sstep(4.2, 2.0, p.distance(STALL + Vec2::new(-1.2, 1.5)));
    let around_hab = sstep(5.0, 3.0, p.distance(HAB)) * 0.7;
    sstep(1.25, 0.45, d).max(around_stall).max(around_hab)
}

/// Linear albedo (before the texture modulation) for ground with normal `n`.
fn ground_albedo(x: f32, z: f32, h: f32, n: Vec3, t: f32) -> Vec3 {
    use noise::*;
    let p = Vec2::new(x, z);
    let r = p.distance(Vec2::new(EYE.x as f32, EYE.z as f32));
    let lod = (r * 0.01).max(0.05);
    // Grassland: golden tussock grass with olive and rust-red patches.
    let patch = fbm_lod(x / 35.0, z / 35.0, 5, lod / 35.0, 31);
    let patch2 = fbm_lod(x / 140.0, z / 140.0, 4, lod / 140.0, 32);
    let gold = hex(0xa88a4c);
    let olive = hex(0x7d7438);
    let rust = hex(0x9a5e3a);
    let mut grass = mix3(gold, olive, sstep(0.0, 0.6, patch) * 0.8);
    grass = mix3(grass, rust, sstep(0.25, 0.6, patch2) * 0.6);
    grass *= 0.92 + 0.16 * fbm_lod(x / 4.0, z / 4.0, 3, lod / 4.0, 33);
    // Rock: blue-grey with rusty strata.
    let strata = ((h / 5.5 + fbm(x / 60.0, z / 60.0, 3, 34) * 1.5) * TAU).sin() * 0.5 + 0.5;
    let rock = mix3(hex(0x5f5d5c), hex(0x7a5c4a), strata * 0.55)
        * (0.8 + 0.3 * fbm_lod(x / 12.0, z / 12.0, 4, lod / 12.0, 35));
    let scree = hex(0x77716a);
    let soil = hex(0x6e573f);

    let slope = 1.0 - n.y;
    let rockiness = sstep(0.16, 0.3, slope + 0.08 * patch);
    let screeness = sstep(0.07, 0.16, slope) * (1.0 - rockiness) * sstep(10.0, 40.0, t);
    let mut c = mix3(grass, scree, screeness * 0.7);
    // Bedrock breaks through at the plateau lip.
    let lip = (-(t / 9.0).powi(2)).exp() * sstep(0.0, 0.3, fbm(x / 6.0, z / 6.0, 3, 36));
    c = mix3(c, rock, rockiness.max(lip * 0.8));
    // Feet wear the path down to soil.
    c = mix3(c, soil, path_wear(p) * 0.9);
    // Snow on the high peaks, on gentler slopes.
    let snowline = 1350.0 + 260.0 * fbm(x / 3000.0, z / 3000.0, 3, 37);
    let snow =
        sstep(snowline - 120.0, snowline + 150.0, h) * sstep(0.35, 0.12, slope - 0.1 * patch);
    c = mix3(c, hex(0xe9edf2), snow);
    c
}

/// The polar terrain grid around the eye.
struct TerrainGrid {
    radii: Vec<f32>,
    angles: Vec<f32>,
    pos: Vec<Vec3>,
}

impl TerrainGrid {
    fn at(&self, i: usize, j: usize) -> Vec3 {
        self.pos[i * self.angles.len() + j]
    }

    /// Ground height under (x, z), from the analytic function (for placing props).
    fn ground(&self, x: f32, z: f32) -> f32 {
        let r = Vec2::new(x - EYE.x as f32, z - EYE.z as f32).length();
        height(x, z, (r * 0.007 * 2.5).max(0.03))
    }
}

const ANGLE_MIN_DEG: f32 = -64.0;
const ANGLE_MAX_DEG: f32 = 60.0;
const ANGLE_STEP_DEG: f32 = 0.4;

fn spawn_terrain(world: &mut World, mats: &Materials) -> TerrainGrid {
    let step = ANGLE_STEP_DEG.to_radians();
    let segs = ((ANGLE_MAX_DEG - ANGLE_MIN_DEG) / ANGLE_STEP_DEG).round() as usize;
    let angles: Vec<f32> = (0..=segs)
        .map(|j| (ANGLE_MIN_DEG + j as f32 * ANGLE_STEP_DEG).to_radians())
        .collect();
    let mut radii = Vec::new();
    let mut r = 1.8f32;
    while r < 32_000.0 {
        radii.push(r);
        let aspect = 1.5 + 1.0 * sstep(120.0, 5000.0, r);
        r *= 1.0 + step * aspect;
    }
    let (ni, nj) = (radii.len(), angles.len());
    let (ex, ez) = (EYE.x as f32, EYE.z as f32);

    // Heights, ring by ring.
    let rows: Vec<Vec<Vec3>> = par_map(ni, |i| {
        let r = radii[i];
        let min_wl = (r * step * 2.5).max(0.03);
        angles
            .iter()
            .map(|&a| {
                let (x, z) = (ex + r * a.sin(), ez - r * a.cos());
                Vec3::new(x, height(x, z, min_wl), z)
            })
            .collect()
    });
    let pos: Vec<Vec3> = rows.into_iter().flatten().collect();
    let grid = TerrainGrid { radii, angles, pos };

    // Normals and colours.
    let shading: Vec<Vec<(Vec3, Vec4)>> = par_map(ni, |i| {
        (0..nj)
            .map(|j| {
                let p = grid.at(i, j);
                let (i0, i1) = (i.saturating_sub(1), (i + 1).min(ni - 1));
                let (j0, j1) = (j.saturating_sub(1), (j + 1).min(nj - 1));
                let dr = grid.at(i1, j) - grid.at(i0, j);
                let da = grid.at(i, j1) - grid.at(i, j0);
                let mut n = da.cross(dr).normalize_or(Vec3::Y);
                if n.y < 0.0 {
                    n = -n;
                }
                // Cavity occlusion from the height above the neighbours' mean, at two scales.
                let mut ao = 1.0;
                for (k, w) in [(2usize, 0.5f32), (7, 0.35)] {
                    let (a0, a1) = (i.saturating_sub(k), (i + k).min(ni - 1));
                    let (b0, b1) = (j.saturating_sub(k), (j + k).min(nj - 1));
                    let mean =
                        (grid.at(a0, j).y + grid.at(a1, j).y + grid.at(i, b0).y + grid.at(i, b1).y)
                            * 0.25;
                    let span = grid.at(a0, j).distance(grid.at(a1, j)).max(0.05);
                    ao *= 1.0 - w * sat((mean - p.y) / span * 4.0);
                }
                let t = past_edge(p.x, p.z);
                let c = ground_albedo(p.x, p.z, p.y, n, t) * (ao / GROUND_TEX_MEAN);
                (n, rgba(c))
            })
            .collect()
    });

    // Chunks of rings × sectors, each its own mesh so culling (camera and shadow cascades)
    // only draws what it needs.
    const RINGS: usize = 40;
    const SECTORS: usize = 52;
    let mut i0 = 0;
    while i0 + 1 < ni {
        let i1 = (i0 + RINGS).min(ni - 1);
        let r_mid = grid.radii[(i0 + i1) / 2];
        let mat = if grid.radii[i0] < 70.0 {
            mats.ground_near
        } else if grid.radii[i0] < 1400.0 {
            mats.ground_mid
        } else {
            mats.ground_far
        };
        let mut j0 = 0;
        while j0 + 1 < nj {
            let j1 = (j0 + SECTORS).min(nj - 1);
            let mut m = Mesh::default();
            let w = j1 - j0 + 1;
            for (i, row) in shading.iter().enumerate().take(i1 + 1).skip(i0) {
                for (j, &(n, c)) in row.iter().enumerate().take(j1 + 1).skip(j0) {
                    let p = grid.at(i, j);
                    m.vertices.push(vtx(p, n, Vec2::new(p.x, p.z), c));
                }
            }
            for i in 0..(i1 - i0) {
                for j in 0..(j1 - j0) {
                    let a = (i * w + j) as u32;
                    let b = a + 1;
                    let c = a + w as u32;
                    let d = c + 1;
                    m.indices.extend_from_slice(&[a, b, c, b, d, c]);
                }
            }
            fix_winding(&mut m);
            let shadows = r_mid < 9000.0;
            spawn_mesh(world, m, mat, Transform::default(), shadows);
            j0 = j1;
        }
        i0 = i1;
    }
    grid
}

/// Worn soil: a decal over the terrain along the path and where people stand (around the
/// stall and the hab door), with ragged alpha-masked edges.
fn spawn_path_decal(world: &mut World, mats: &Materials) {
    let cell = 0.25f32;
    let (x0, x1, z0, z1) = (-18.0f32, 20.0f32, -60.0f32, 9.0f32);
    let (nx, nz) = (((x1 - x0) / cell) as usize, ((z1 - z0) / cell) as usize);
    let wear: Vec<Vec<f32>> = par_map(nz + 1, |k| {
        (0..=nx)
            .map(|i| path_wear(Vec2::new(x0 + i as f32 * cell, z0 + k as f32 * cell)))
            .collect()
    });
    let mut m = Mesh::default();
    let mut index: HashMap<(usize, usize), u32> = HashMap::new();
    let mut vert = |m: &mut Mesh, i: usize, k: usize| -> u32 {
        *index.entry((i, k)).or_insert_with(|| {
            let (x, z) = (x0 + i as f32 * cell, z0 + k as f32 * cell);
            let e = 0.05;
            let h = height(x, z, 0.03);
            let dx = height(x + e, z, 0.03) - height(x - e, z, 0.03);
            let dz = height(x, z + e, 0.03) - height(x, z - e, 0.03);
            let n = Vec3::new(-dx, 2.0 * e, -dz).normalize();
            let a = sat(wear[k][i] * 1.25);
            let c = Vec4::new(0.5, 0.37, 0.25, a);
            m.vertices
                .push(vtx(Vec3::new(x, h + 0.012, z), n, Vec2::new(x, z), c));
            m.vertices.len() as u32 - 1
        })
    };
    for k in 0..nz {
        for i in 0..nx {
            let w = wear[k][i]
                .max(wear[k][i + 1])
                .max(wear[k + 1][i])
                .max(wear[k + 1][i + 1]);
            if w < 0.25 {
                continue;
            }
            let a = vert(&mut m, i, k);
            let b = vert(&mut m, i + 1, k);
            let c = vert(&mut m, i, k + 1);
            let d = vert(&mut m, i + 1, k + 1);
            m.indices.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    fix_winding(&mut m);
    spawn_mesh(world, m, mats.dirt, Transform::default(), false);
}

// ============================================================================ meshes

/// Unit icosphere: vertices and triangles.
fn icosphere(subdiv: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let t = (1.0 + 5.0f32.sqrt()) / 2.0;
    let mut v: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .iter()
    .map(|p| Vec3::from(*p).normalize())
    .collect();
    let mut f: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdiv {
        let mut cache: HashMap<(u32, u32), u32> = HashMap::new();
        let mut mid = |a: u32, b: u32, v: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *cache.entry(key).or_insert_with(|| {
                v.push(((v[a as usize] + v[b as usize]) * 0.5).normalize());
                v.len() as u32 - 1
            })
        };
        let mut nf = Vec::with_capacity(f.len() * 4);
        for [a, b, c] in f {
            let ab = mid(a, b, &mut v);
            let bc = mid(b, c, &mut v);
            let ca = mid(c, a, &mut v);
            nf.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        f = nf;
    }
    (v, f)
}

/// A boulder: a chunky body cut by fracture planes (crisp, slightly rounded edges), chipped
/// along its edges, with tilted bedding ledges and cracks, roughened with noise. Cavity
/// occlusion and edge wear (from curvature), dust on top and grime at the base are baked
/// into vertex colours. UVs are box projected per triangle (metres), for the rock texture.
fn rock_mesh(seed: u32, subdiv: u32, size: Vec3, tint_c: Vec3) -> Mesh {
    let (dirs, faces) = icosphere(subdiv);
    let mut rng = Rng::new(seed as u64 * 7 + 3);
    // (normal, offset, edge rounding): big fractures, a tilted flattish top, then chips.
    let mut planes: Vec<(Vec3, f32, f32)> = (0..9)
        .map(|_| (rng.unit_vector(), rng.range(0.68, 0.9), 0.04))
        .collect();
    let top = Vec3::new(rng.range(-0.35, 0.35), 1.0, rng.range(-0.35, 0.35)).normalize();
    planes.push((top, rng.range(0.7, 0.86), 0.06));
    for _ in 0..18 {
        planes.push((rng.unit_vector(), rng.range(0.8, 0.95), 0.012));
    }
    let bedding = (Vec3::Y + rng.unit_vector() * 0.45).normalize();
    let layers = rng.range(4.5, 7.0);
    let s = seed as f32 * 1.37;
    let radius: Vec<f32> = dirs
        .iter()
        .map(|&d| {
            use noise::{fbm3, n3};
            let q = d * 1.3 + Vec3::splat(s);
            // Warp the direction a little so the cuts are not perfectly flat.
            let warp = Vec3::new(
                n3(q.x, q.y, q.z, seed + 3),
                n3(q.y, q.z, q.x, seed + 4),
                n3(q.z, q.x, q.y, seed + 5),
            ) * 0.07;
            let dw = (d + warp).normalize();
            let mut r = 1.2f32;
            for &(n, o, k) in &planes {
                let c = dw.dot(n);
                if c > 0.02 {
                    r = smin(r, o / c, k);
                }
            }
            r *= 1.0 + 0.07 * fbm3(q.x, q.y, q.z, 3, seed);
            // Bedding: soft ledges where the layers meet the surface.
            let lp =
                d.dot(bedding) * r * layers + 0.3 * n3(q.x * 2.0, q.y * 2.0, q.z * 2.0, seed + 6);
            r -= 0.012 * (0.5 + 0.5 * (lp * TAU).sin()).powi(2);
            // Fine roughness.
            let q4 = d * 9.0 + Vec3::splat(s);
            r += 0.009 * fbm3(q4.x, q4.y, q4.z, 3, seed + 9);
            r
        })
        .collect();
    let pos: Vec<Vec3> = dirs
        .iter()
        .zip(&radius)
        .map(|(&d, &r)| {
            let mut p = d * r * size;
            // Sit flat-ish on the ground.
            if p.y < -0.35 * size.y {
                p.y = -0.35 * size.y + (p.y + 0.35 * size.y) * 0.3;
            }
            p.y = p.y.max(-0.42 * size.y);
            p
        })
        .collect();
    let mut m = Mesh {
        vertices: pos
            .iter()
            .map(|&p| vtx(p, Vec3::Y, Vec2::ZERO, Vec4::ONE))
            .collect(),
        indices: faces.iter().flat_map(|f| f.iter().copied()).collect(),
    };
    m.compute_normals();
    // Curvature from the umbrella operator: concave = cavity, convex = worn edge.
    let mut sum = vec![Vec3::ZERO; pos.len()];
    let mut cnt = vec![0.0f32; pos.len()];
    let mut edge_len = 0.0;
    for f in &faces {
        for k in 0..3 {
            let (a, b) = (f[k] as usize, f[(k + 1) % 3] as usize);
            sum[a] += pos[b];
            cnt[a] += 1.0;
            sum[b] += pos[a];
            cnt[b] += 1.0;
            edge_len += pos[a].distance(pos[b]);
        }
    }
    let edge_len = edge_len / (faces.len() * 3) as f32;
    let hue = rng.f32();
    for (i, v) in m.vertices.iter_mut().enumerate() {
        let n = Vec3::from(v.normal);
        let p = Vec3::from(v.position);
        let lap = sum[i] / cnt[i].max(1.0) - p;
        let curv = lap.dot(n) / edge_len;
        let cavity = sat(curv * 3.0);
        let edge = sat(-curv * 2.5);
        let q = p / size.max_element() * 3.0 + Vec3::splat(s);
        let stain = noise::fbm3(q.x, q.y, q.z, 3, seed + 11);
        let mut c = tint_c * (1.0 - 0.6 * cavity) * (1.0 + 0.3 * edge);
        // Iron staining in patches, stronger in some boulders.
        c = mix3(
            c,
            c * Vec3::new(1.25, 0.9, 0.62),
            sat(stain * 1.5 + 0.2) * (0.25 + 0.5 * hue),
        );
        // Dust settles on what faces up, grime and soil splash at the base.
        let up = sstep(0.5, 0.95, n.y) * (1.0 - cavity);
        c = mix3(c, Vec3::new(0.86, 0.72, 0.52), up * 0.35);
        let ground = sstep(-0.12 * size.y, -0.4 * size.y, p.y);
        c = mix3(c, c * Vec3::new(0.5, 0.42, 0.32), ground);
        v.color = rgba(c).to_array();
    }
    box_uv_unshared(&m)
}

/// Give every triangle its own vertices with UVs projected along its dominant axis (metres),
/// keeping the smooth normals.
fn box_uv_unshared(m: &Mesh) -> Mesh {
    let mut out = Mesh::default();
    for tri in m.indices.as_chunks::<3>().0 {
        let p: Vec<Vec3> = tri
            .iter()
            .map(|&i| Vec3::from(m.vertices[i as usize].position))
            .collect();
        let g = (p[1] - p[0]).cross(p[2] - p[0]).abs();
        for (k, &i) in tri.iter().enumerate() {
            let mut v = m.vertices[i as usize];
            let q = p[k];
            let uv = if g.x >= g.y && g.x >= g.z {
                Vec2::new(q.z, q.y)
            } else if g.y >= g.z {
                Vec2::new(q.x, q.z)
            } else {
                Vec2::new(q.x, q.y)
            };
            v.uv = uv.to_array();
            out.vertices.push(v);
        }
    }
    out.indices = (0..out.vertices.len() as u32).collect();
    out
}

/// Box with rounded edges (radius `r`), centred on the origin. UVs are in metres with V along
/// axis `grain` (0 = x, 1 = y, 2 = z), so wood grain follows the long side.
fn bevel_box(size: Vec3, r: f32, grain: usize) -> Mesh {
    let h = size * 0.5;
    let r = r.min(h.min_element() * 0.9).max(0.0005);
    const SEGS: usize = 3;
    let samples = |half: f32| -> Vec<f32> {
        let mut s = Vec::new();
        let inner = half - r;
        for k in (1..=SEGS).rev() {
            s.push(-(inner + r * (k as f32 / SEGS as f32 * FRAC_PI_4).tan()));
        }
        let n = ((2.0 * inner / 0.35).ceil() as usize).max(1);
        for k in 0..=n {
            s.push(-inner + 2.0 * inner * k as f32 / n as f32);
        }
        for k in 1..=SEGS {
            s.push(inner + r * (k as f32 / SEGS as f32 * FRAC_PI_4).tan());
        }
        s
    };
    let inner = h - Vec3::splat(r);
    let mut m = Mesh::default();
    for a in 0..3 {
        let (b, c) = ((a + 1) % 3, (a + 2) % 3);
        let (sb, sc) = (samples(h[b]), samples(h[c]));
        for sign in [-1.0f32, 1.0] {
            let base = m.vertices.len() as u32;
            for &u in &sb {
                for &v in &sc {
                    let mut p = Vec3::ZERO;
                    p[a] = sign * h[a];
                    p[b] = u;
                    p[c] = v;
                    let q = p.clamp(-inner, inner);
                    let n = (p - q).normalize_or(Vec3::Y);
                    let pos = q + n * r;
                    let uv = if grain == b {
                        Vec2::new(v, u)
                    } else {
                        Vec2::new(u, v)
                    };
                    m.vertices.push(vtx(pos, n, uv, Vec4::ONE));
                }
            }
            let nc = sc.len() as u32;
            for k in 0..sb.len() as u32 - 1 {
                for l in 0..nc - 1 {
                    let i00 = base + k * nc + l;
                    let i10 = i00 + nc;
                    m.indices
                        .extend_from_slice(&[i00, i10, i10 + 1, i00, i10 + 1, i00 + 1]);
                }
            }
        }
    }
    fix_winding(&mut m);
    m
}

/// A beam between two points (rounded box, grain along its length).
fn beam(a: Vec3, b: Vec3, w: f32, d: f32, r: f32) -> Mesh {
    let len = a.distance(b);
    let m = bevel_box(Vec3::new(w, len, d), r, 1);
    let rot = Quat::from_rotation_arc(Vec3::Y, (b - a) / len);
    moved(m, Mat4::from_rotation_translation(rot, (a + b) * 0.5))
}

/// Surface of revolution around +Y. `profile` holds (radius, y) from bottom to top. UVs: u
/// around the circumference in metres at radius `u_radius`, v = y in metres.
fn lathe(profile: &[(f32, f32)], segments: u32, u_radius: f32) -> Mesh {
    let mut m = Mesh::default();
    let n = profile.len();
    let row = segments + 1;
    for (k, &(r, y)) in profile.iter().enumerate() {
        let prev = profile[k.saturating_sub(1)];
        let next = profile[(k + 1).min(n - 1)];
        let t = Vec2::new(next.0 - prev.0, next.1 - prev.1);
        let n2 = Vec2::new(t.y, -t.x).normalize_or(Vec2::X);
        for s in 0..=segments {
            let a = s as f32 / segments as f32 * TAU;
            let (sin, cos) = a.sin_cos();
            let p = Vec3::new(cos * r, y, -sin * r);
            let nn = Vec3::new(cos * n2.x, n2.y, -sin * n2.x);
            m.vertices
                .push(vtx(p, nn, Vec2::new(a * u_radius, y), Vec4::ONE));
        }
    }
    for k in 0..n as u32 - 1 {
        for s in 0..segments {
            let i = k * row + s;
            m.indices
                .extend_from_slice(&[i, i + 1, i + row, i + 1, i + row + 1, i + row]);
        }
    }
    fix_winding(&mut m);
    m
}

/// A tube between two points.
fn tube(a: Vec3, b: Vec3, radius: f32, segments: u32) -> Mesh {
    let len = a.distance(b);
    let m = lathe(
        &[(0.0, 0.0), (radius, 0.0), (radius, len), (0.0, len)],
        segments,
        radius,
    );
    let rot = Quat::from_rotation_arc(Vec3::Y, (b - a) / len);
    moved(m, Mat4::from_rotation_translation(rot, a))
}

/// A crystal: a six-sided prism with a pointed tip, faceted.
fn crystal(len: f32, r: f32) -> Mesh {
    let mut m = lathe(
        &[
            (0.0, -0.02),
            (r * 0.8, 0.0),
            (r, len * 0.15),
            (r * 0.92, len * 0.72),
            (0.0, len),
        ],
        6,
        r,
    );
    m.compute_flat_normals();
    m
}

// ============================================================================ rocks

/// The turn (about +Y, near `yaw`) that shows the camera about half of `m` in sunlight and
/// half in shade, so the boulder reads by its form.
fn lit_yaw(m: &Mesh, at: Vec3, yaw: f32) -> f32 {
    let to_eye = (EYE.as_vec3() - at).normalize();
    let sun = sun_dir();
    let tris: Vec<(Vec3, f32)> = m
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let p = |i: u32| Vec3::from(m.vertices[i as usize].position);
            let n = |i: u32| Vec3::from(m.vertices[i as usize].normal);
            let c = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            let c = if c.dot(n(t[0]) + n(t[1]) + n(t[2])) < 0.0 {
                -c
            } else {
                c
            };
            (c.normalize_or(Vec3::Y), c.length() * 0.5)
        })
        .collect();
    let score = |a: f32| {
        let q = Quat::from_rotation_y(a);
        let (mut seen, mut lit) = (0.0, 0.0);
        for &(n, area) in &tris {
            let n = q * n;
            let v = n.dot(to_eye).max(0.0) * area;
            seen += v;
            lit += v * sstep(0.0, 0.25, n.dot(sun));
        }
        (lit / seen.max(1e-6) - 0.55).abs()
    };
    (0..16)
        .map(|k| yaw + k as f32 * TAU / 16.0)
        .min_by(|a, b| score(*a).total_cmp(&score(*b)))
        .unwrap_or(yaw)
}

struct PlacedRock {
    at: Vec2,
    radius: f32,
}

fn spawn_rocks(world: &mut World, mats: &Materials, terrain: &TerrainGrid) -> Vec<PlacedRock> {
    // Hand-placed boulders: (x, z, size, yaw, seed, detail).
    let hero: [(f32, f32, Vec3, f32, u32, u32); 12] = [
        (-4.6, -1.6, Vec3::new(1.7, 1.35, 1.4), 0.4, 1, 6),
        (-3.0, -0.4, Vec3::new(0.6, 0.5, 0.55), 1.2, 2, 5),
        (-10.5, -17.0, Vec3::new(2.6, 1.9, 2.2), 2.1, 3, 5),
        (-12.5, -14.8, Vec3::new(1.0, 0.7, 0.9), 0.3, 4, 4),
        (15.5, -21.0, Vec3::new(2.2, 1.5, 1.8), 1.0, 5, 5),
        (9.4, -4.2, Vec3::new(0.5, 0.35, 0.45), 2.6, 6, 4),
        (-7.5, -40.0, Vec3::new(4.5, 2.4, 3.2), 0.8, 7, 5),
        (6.0, -47.0, Vec3::new(5.5, 2.0, 3.0), 2.2, 8, 5),
        (-20.0, -52.0, Vec3::new(3.6, 2.6, 3.0), 1.7, 9, 4),
        (22.0, -42.0, Vec3::new(3.0, 1.8, 2.6), 0.2, 10, 4),
        (34.0, -70.0, Vec3::new(4.0, 2.2, 3.4), 1.1, 11, 4),
        (-30.0, -30.0, Vec3::new(2.0, 1.4, 1.7), 2.9, 12, 4),
    ];
    let mut placed = Vec::new();
    let mut rng = Rng::new(77);
    for (x, z, size, yaw, seed, detail) in hero {
        let tone = rng.range(0.85, 1.1);
        let c = Vec3::new(
            tone,
            tone * rng.range(0.93, 0.98),
            tone * rng.range(0.84, 0.92),
        );
        let m = rock_mesh(seed, detail, size, c);
        let y = terrain.ground(x, z) - 0.12 * size.y;
        let yaw = lit_yaw(&m, Vec3::new(x, y, z), yaw);
        spawn_mesh(
            world,
            m,
            mats.rock,
            Transform::from_xyz(x as f64, y as f64, z as f64)
                .with_rotation(Quat::from_rotation_y(yaw)),
            true,
        );
        placed.push(PlacedRock {
            at: Vec2::new(x, z),
            radius: size.x.max(size.z) * 0.75,
        });
    }
    // Scattered stones on the plateau (a few shared meshes, instanced).
    let variants: Vec<Handle<Mesh>> = (0..6)
        .map(|k| {
            let m = rock_mesh(
                100 + k,
                2,
                Vec3::new(1.0, 0.65, 0.85),
                Vec3::new(1.0, 0.93, 0.85),
            );
            add_mesh(world, m)
        })
        .collect();
    let mut n = 0;
    while n < 220 {
        let a = rng.range(ANGLE_MIN_DEG, ANGLE_MAX_DEG).to_radians();
        let r = 2.5 + rng.f32().powf(1.6) * 70.0;
        let (x, z) = (EYE.x as f32 + r * a.sin(), EYE.z as f32 - r * a.cos());
        let p = Vec2::new(x, z);
        if past_edge(x, z) > -3.0 || (calm(p) > 0.5 && path_distance(p) > 1.4) {
            continue;
        }
        // Stones gather along the path's edges and around the boulders.
        let near_path = sstep(3.0, 1.2, path_distance(p)) * sstep(0.5, 1.0, path_distance(p));
        if !rng.chance(0.25 + 0.75 * near_path) {
            continue;
        }
        let s = rng.range(0.03, 0.12) * if rng.chance(0.12) { 2.0 } else { 1.0 };
        let y = terrain.ground(x, z) - s * 0.3;
        spawn_handle(
            world,
            variants[n % variants.len()],
            mats.rock,
            Transform::from_xyz(x as f64, y as f64, z as f64)
                .with_rotation(Quat::from_euler(
                    EulerRot::YXZ,
                    rng.range(0.0, TAU),
                    rng.range(-0.2, 0.2),
                    rng.range(-0.2, 0.2),
                ))
                .with_uniform_scale(s),
            s > 0.15,
        );
        n += 1;
    }
    placed
}

// ============================================================================ grass

/// Add a tussock (a fountain of long, thin, arching blades, with a few seed stalks) at `c` to
/// `m`. `lod` 0 is the most detailed. Blades are wound to face the camera, with normals bent
/// toward the sky, so they shade softly without a double-sided material.
fn tussock(m: &mut Mesh, rng: &mut Rng, c: Vec3, scale: f32, lod: usize) {
    let eye = EYE.as_vec3();
    let blades = [72, 40, 22][lod];
    let segs = [4, 3, 2][lod];
    let width_k = [1.0, 1.5, 2.4][lod];
    // Keep blades at least about a pixel wide at 1080p, so they don't break up into dashes.
    let min_half_w = eye.distance(c) * 0.0008;
    let hue = rng.f32();
    let green = rng.f32() * 0.35;
    let gold = mix3(hex(0x9c7838), hex(0xb48c4c), hue);
    let strip =
        |m: &mut Mesh, pts: &[(Vec3, f32)], side: Vec3, w0: f32, base_c: Vec3, tip_c: Vec3| {
            let base = m.vertices.len() as u32;
            for (k, &(q, s)) in pts.iter().enumerate() {
                let tangent = if k + 1 < pts.len() {
                    pts[k + 1].0 - q
                } else {
                    q - pts[k - 1].0
                };
                let mut n = side.cross(tangent).normalize_or(Vec3::Y);
                if n.dot(eye - q) < 0.0 {
                    n = -n;
                }
                let n = (n * 0.6 + Vec3::Y * 0.55).normalize();
                let w = w0 * (1.0 - s.powf(1.4)).max(0.0) + 0.0004;
                let ao = 0.22 + 0.78 * s.powf(0.55);
                let col = rgba(mix3(base_c, tip_c, s.powf(0.7)) * ao);
                m.vertices
                    .push(vtx(q - side * w, n, Vec2::new(0.0, s), col));
                m.vertices
                    .push(vtx(q + side * w, n, Vec2::new(1.0, s), col));
            }
            for k in 0..pts.len() as u32 - 1 {
                let i = base + k * 2;
                for t in [[i, i + 1, i + 2], [i + 1, i + 3, i + 2]] {
                    let pa = Vec3::from(m.vertices[t[0] as usize].position);
                    let pb = Vec3::from(m.vertices[t[1] as usize].position);
                    let pc = Vec3::from(m.vertices[t[2] as usize].position);
                    if (pb - pa).cross(pc - pa).dot(eye - pa) >= 0.0 {
                        m.indices.extend_from_slice(&t);
                    } else {
                        m.indices.extend_from_slice(&[t[0], t[2], t[1]]);
                    }
                }
            }
        };
    let mut pts = Vec::with_capacity(8);
    for _ in 0..blades {
        let root_d = rng.in_unit_disk() * 0.07 * scale;
        let root = c + Vec3::new(root_d.x, -0.03, root_d.y);
        let az = rng.range(0.0, TAU);
        let out = root_d.length() / (0.07 * scale);
        let lean0 = rng.range(0.03, 0.3) + 0.35 * out;
        let bend = rng.range(0.6, 2.0) * (0.5 + 0.7 * out);
        let len = rng.range(0.3, 0.78) * scale * (1.0 - 0.25 * out);
        let w0 = (rng.range(0.0035, 0.0055) * width_k * scale.sqrt()).max(min_half_w);
        let dir_h = Vec3::new(az.cos(), 0.0, az.sin());
        let side = Vec3::new(az.sin(), 0.0, -az.cos());
        let kind = rng.f32();
        let (base_c, tip_c) = if kind < green {
            (hex(0x2c3012), hex(0x8e9048))
        } else if kind > 0.92 {
            (hex(0x2e2820), hex(0x9a8e7c))
        } else {
            (gold * 0.3, mix3(gold, hex(0xe0c890), 0.55))
        };
        pts.clear();
        let mut p = root;
        for k in 0..=segs {
            let s = k as f32 / segs as f32;
            pts.push((p, s));
            let lean = (lean0 + bend * s * s).min(2.6);
            let d = Vec3::Y * lean.cos() + dir_h * lean.sin();
            p += d * (len / segs as f32);
        }
        strip(m, &pts, side, w0, base_c, tip_c);
    }
    // Seed stalks: straight, thin, catching the light above the clump.
    if lod == 0 {
        for _ in 0..rng.range_i32(0, 3) {
            let az = rng.range(0.0, TAU);
            let dir_h = Vec3::new(az.cos(), 0.0, az.sin());
            let side = Vec3::new(az.sin(), 0.0, -az.cos());
            let tilt = rng.range(0.05, 0.35);
            let len = rng.range(0.55, 0.95) * scale;
            let root = c + Vec3::new(0.0, -0.02, 0.0);
            let d = Vec3::Y * tilt.cos() + dir_h * tilt.sin();
            pts.clear();
            for k in 0..=3 {
                let s = k as f32 / 3.0;
                pts.push((root + d * len * s + dir_h * (0.04 * s * s * scale), s));
            }
            strip(
                m,
                &pts,
                side,
                (0.0024 * width_k).max(min_half_w),
                hex(0x3a2c18),
                hex(0xa48a5a),
            );
        }
    }
}

fn spawn_grass(world: &mut World, mats: &Materials, rocks: &[PlacedRock]) {
    let eye = Vec2::new(EYE.x as f32, EYE.z as f32);
    let mut cells: HashMap<(i32, i32), Mesh> = HashMap::new();
    let mut rng = Rng::new(9);
    let blocked = |p: Vec2, pad: f32| {
        past_edge(p.x, p.y) > -5.0
            || rocks.iter().any(|r| r.at.distance(p) < r.radius + pad)
            || p.distance(STALL) < 2.6
            || p.distance(HAB) < 3.6
    };
    let in_view = |p: Vec2| {
        let ang = (p.x - eye.x).atan2(eye.y - p.y).to_degrees();
        (-54.0..52.0).contains(&ang)
    };
    // Tussocks.
    let spacing = 0.6;
    let ext = 66.0;
    let n = (2.0 * ext / spacing) as i32;
    for gz in 0..n {
        for gx in 0..n {
            let base = Vec2::new(
                -ext + gx as f32 * spacing,
                4.0 - 2.0 * ext + gz as f32 * spacing,
            );
            let p = base + Vec2::new(rng.f32(), rng.f32()) * spacing;
            let d = p.distance(eye);
            if !(1.5..ext).contains(&d) || !in_view(p) {
                continue;
            }
            // Thin out with distance; farther tussocks get bigger and coarser.
            let keep = (4.5 / d.max(4.5)).powf(1.15);
            if !rng.chance(keep) {
                continue;
            }
            let wear = path_wear(p);
            let patchy = noise::fbm(p.x / 9.0, p.y / 9.0, 3, 300);
            if wear > 0.3 || blocked(p, 0.2) || !rng.chance(sstep(-0.55, 0.0, patchy)) {
                continue;
            }
            let y = height(p.x, p.y, (d * d * 0.0007).max(0.03));
            let lod = if d < 11.0 {
                0
            } else if d < 26.0 {
                1
            } else {
                2
            };
            let grow = (d / 4.5).max(1.0).powf(0.42).min(2.6);
            let scale = rng.range(0.8, 1.2) * grow * (1.0 - 0.4 * wear);
            let key = ((p.x / 12.0).floor() as i32, (p.y / 12.0).floor() as i32);
            let m = cells.entry(key).or_default();
            tussock(m, &mut rng, Vec3::new(p.x, y, p.y), scale, lod);
        }
    }
    // Short tufts between them near the eye.
    let spacing = 0.22;
    let ext = 16.0;
    let n = (2.0 * ext / spacing) as i32;
    for gz in 0..n {
        for gx in 0..n {
            let base = Vec2::new(
                -ext + gx as f32 * spacing,
                4.0 - 2.0 * ext + gz as f32 * spacing,
            );
            let p = base + Vec2::new(rng.f32(), rng.f32()) * spacing;
            let d = p.distance(eye);
            if !(1.5..ext).contains(&d) || !in_view(p) || !rng.chance((5.0 / d.max(5.0)).powi(2)) {
                continue;
            }
            let wear = path_wear(p);
            if wear > 0.55 || blocked(p, 0.05) {
                continue;
            }
            let y = height(p.x, p.y, 0.03);
            let key = ((p.x / 12.0).floor() as i32, (p.y / 12.0).floor() as i32);
            let m = cells.entry(key).or_default();
            tuft(m, &mut rng, Vec3::new(p.x, y, p.y), 1.0 - 0.6 * wear);
        }
    }
    let mut keys: Vec<_> = cells.keys().copied().collect();
    keys.sort();
    for k in keys {
        if let Some(m) = cells.remove(&k) {
            spawn_mesh(world, m, mats.grass, Transform::default(), false);
        }
    }
}

/// A few short blades (a low tuft), wound toward the camera like the tussocks.
fn tuft(m: &mut Mesh, rng: &mut Rng, c: Vec3, scale: f32) {
    let eye = EYE.as_vec3();
    let green = rng.chance(0.3);
    for _ in 0..rng.range_i32(4, 9) {
        let off = rng.in_unit_disk() * 0.05;
        let root = c + Vec3::new(off.x, -0.01, off.y);
        let az = rng.range(0.0, TAU);
        let dir_h = Vec3::new(az.cos(), 0.0, az.sin());
        let side = Vec3::new(az.sin(), 0.0, -az.cos());
        let len = rng.range(0.06, 0.2) * scale;
        let lean = rng.range(0.2, 0.9);
        let mid = root + (Vec3::Y * lean.cos() + dir_h * lean.sin()) * len * 0.55;
        let tip = mid + (Vec3::Y * (lean + 0.5).cos() + dir_h * (lean + 0.5).sin()) * len * 0.45;
        let w = rng.range(0.004, 0.007);
        let (base_c, tip_c) = if green {
            (hex(0x2f3416), hex(0x8c9446))
        } else {
            (hex(0x4a3a1e), hex(0xc4a466))
        };
        let n0 = side.cross(mid - root).normalize_or(Vec3::Y);
        let n0 = if n0.dot(eye - root) < 0.0 { -n0 } else { n0 };
        let n = (n0 * 0.4 + Vec3::Y).normalize();
        let b = m.vertices.len() as u32;
        m.vertices
            .push(vtx(root - side * w, n, Vec2::ZERO, rgba(base_c * 0.4)));
        m.vertices
            .push(vtx(root + side * w, n, Vec2::ZERO, rgba(base_c * 0.4)));
        m.vertices.push(vtx(
            mid - side * w * 0.7,
            n,
            Vec2::ZERO,
            rgba(mix3(base_c, tip_c, 0.6)),
        ));
        m.vertices.push(vtx(
            mid + side * w * 0.7,
            n,
            Vec2::ZERO,
            rgba(mix3(base_c, tip_c, 0.6)),
        ));
        m.vertices.push(vtx(tip, n, Vec2::ZERO, rgba(tip_c)));
        for t in [
            [b, b + 1, b + 2],
            [b + 1, b + 3, b + 2],
            [b + 2, b + 3, b + 4],
        ] {
            let pa = Vec3::from(m.vertices[t[0] as usize].position);
            let pb = Vec3::from(m.vertices[t[1] as usize].position);
            let pc = Vec3::from(m.vertices[t[2] as usize].position);
            if (pb - pa).cross(pc - pa).dot(eye - pa) >= 0.0 {
                m.indices.extend_from_slice(&t);
            } else {
                m.indices.extend_from_slice(&[t[0], t[2], t[1]]);
            }
        }
    }
}

// ============================================================================ the stall

/// A crate of planks: frame, slats and lid.
fn crate_mesh(size: Vec3, rng: &mut Rng) -> Mesh {
    let mut m = Mesh::default();
    let h = size * 0.5;
    let t = 0.035;
    let tone = rng.range(0.85, 1.1);
    let col = Vec4::new(tone * 1.05, tone, tone * 0.92, 1.0);
    // Corner posts.
    for sx in [-1.0, 1.0] {
        for sz in [-1.0, 1.0] {
            let c = Vec3::new(sx * (h.x - t), 0.0, sz * (h.z - t));
            m.merge(&tint(
                moved(
                    bevel_box(Vec3::new(t * 2.0, size.y, t * 2.0), 0.006, 1),
                    Mat4::from_translation(c),
                ),
                col * 0.9,
            ));
        }
    }
    // Side slats.
    let slats = 4;
    for k in 0..slats {
        let y = -h.y + (k as f32 + 0.5) * size.y / slats as f32;
        let sh = size.y / slats as f32 * 0.86;
        let jitter = rng.range(0.9, 1.08);
        for sz in [-1.0, 1.0] {
            m.merge(&tint(
                moved(
                    bevel_box(Vec3::new(size.x - 0.01, sh, 0.022), 0.005, 0),
                    Mat4::from_translation(Vec3::new(0.0, y, sz * (h.z - 0.011))),
                ),
                col * jitter,
            ));
        }
        for sx in [-1.0, 1.0] {
            m.merge(&tint(
                moved(
                    bevel_box(Vec3::new(0.022, sh, size.z - 0.01), 0.005, 2),
                    Mat4::from_translation(Vec3::new(sx * (h.x - 0.011), y, 0.0)),
                ),
                col * jitter * 0.97,
            ));
        }
    }
    // Lid planks.
    let planks = 3;
    for k in 0..planks {
        let z = -h.z + (k as f32 + 0.5) * size.z / planks as f32;
        m.merge(&tint(
            moved(
                bevel_box(
                    Vec3::new(size.x, 0.025, size.z / planks as f32 * 0.94),
                    0.005,
                    0,
                ),
                Mat4::from_translation(Vec3::new(0.0, h.y - 0.012, z)),
            ),
            col * rng.range(0.9, 1.1),
        ));
    }
    m
}

/// A barrel: bulging staves (wood) and iron hoops (returned separately).
fn barrel_mesh(height: f32, radius: f32) -> (Mesh, Mesh) {
    let prof: Vec<(f32, f32)> = (0..=16)
        .map(|k| {
            let t = k as f32 / 16.0;
            let bulge = 1.0 - (2.0 * t - 1.0).powi(2);
            (radius * (0.86 + 0.14 * bulge), t * height)
        })
        .collect();
    let mut wood = lathe(&prof, 40, radius);
    // Lid.
    wood.merge(&lathe(
        &[
            (radius * 0.84, height - 0.03),
            (radius * 0.84, height - 0.015),
            (0.0, height - 0.015),
        ],
        40,
        radius,
    ));
    let mut iron = Mesh::default();
    for t in [0.08f32, 0.3, 0.7, 0.92] {
        let bulge = 1.0 - (2.0 * t - 1.0).powi(2);
        let r = radius * (0.86 + 0.14 * bulge) + 0.004;
        let y = t * height;
        iron.merge(&lathe(
            &[
                (r - 0.004, y - 0.025),
                (r, y - 0.022),
                (r, y + 0.022),
                (r - 0.004, y + 0.025),
            ],
            40,
            r,
        ));
    }
    (wood, iron)
}

/// A sack: a squashed, lumpy sphere.
fn sack_mesh(size: Vec3, seed: u32) -> Mesh {
    let (dirs, faces) = icosphere(3);
    let mut m = Mesh {
        vertices: dirs
            .iter()
            .map(|&d| {
                let lump = 1.0 + 0.08 * noise::n3(d.x * 2.5, d.y * 2.5, d.z * 2.5, seed);
                let mut p = d * lump * size;
                if p.y < -0.6 * size.y {
                    p.y = -0.6 * size.y;
                }
                // Tied neck.
                if d.y > 0.7 {
                    let k = sstep(0.7, 1.0, d.y);
                    p.x *= 1.0 - 0.7 * k;
                    p.z *= 1.0 - 0.7 * k;
                    p.y += 0.08 * k * size.y;
                }
                let uv = Vec2::new(d.x.atan2(d.z) * 0.3, d.y * 0.4);
                vtx(p, d, uv, Vec4::new(0.95, 0.85, 0.65, 1.0))
            })
            .collect(),
        indices: faces.iter().flat_map(|f| f.iter().copied()).collect(),
    };
    m.compute_normals();
    m
}

fn spawn_stall(world: &mut World, mats: &Materials) {
    let ground = height(STALL.x, STALL.y, 0.03);
    let rot = Quat::from_rotation_y(STALL_YAW);
    let at = Transform::from_xyz(STALL.x as f64, ground as f64, STALL.y as f64).with_rotation(rot);
    let mut rng = Rng::new(42);
    let weathered = Vec4::new(0.92, 0.9, 0.88, 1.0);

    // Frame: four posts (back ones taller), beams and braces.
    let (hw, hd) = (1.6, 1.0);
    let (front_h, back_h) = (2.3, 2.75);
    let mut wood = Mesh::default();
    for (x, z, hgt) in [
        (-hw, hd, front_h),
        (hw, hd, front_h),
        (-hw, -hd, back_h),
        (hw, -hd, back_h),
    ] {
        let lean = Vec3::new(rng.range(-0.02, 0.02), 0.0, rng.range(-0.02, 0.02));
        wood.merge(&tint(
            beam(
                Vec3::new(x, -0.3, z),
                Vec3::new(x, hgt + 0.08, z) + lean,
                0.12,
                0.12,
                0.018,
            ),
            weathered * rng.range(0.85, 1.0),
        ));
    }
    for (a, b) in [
        (
            Vec3::new(-hw - 0.15, front_h, hd),
            Vec3::new(hw + 0.15, front_h, hd),
        ),
        (
            Vec3::new(-hw - 0.15, back_h, -hd),
            Vec3::new(hw + 0.15, back_h, -hd),
        ),
        (
            Vec3::new(-hw, front_h, hd + 0.1),
            Vec3::new(-hw, back_h, -hd - 0.1),
        ),
        (
            Vec3::new(hw, front_h, hd + 0.1),
            Vec3::new(hw, back_h, -hd - 0.1),
        ),
        (
            Vec3::new(-hw, 1.5, -hd),
            Vec3::new(-hw + 0.7, back_h - 0.05, -hd),
        ),
        (
            Vec3::new(hw, 1.5, -hd),
            Vec3::new(hw - 0.7, back_h - 0.05, -hd),
        ),
    ] {
        wood.merge(&tint(
            beam(a, b, 0.1, 0.09, 0.015),
            weathered * rng.range(0.85, 1.0),
        ));
    }
    // Counter: trestles, a thick top and a front of vertical boards.
    let counter_z = hd - 0.25;
    let top_y = 1.0;
    wood.merge(&tint(
        moved(
            bevel_box(Vec3::new(2.9, 0.07, 0.62), 0.015, 0),
            Mat4::from_translation(Vec3::new(0.0, top_y, counter_z - 0.05)),
        ),
        Vec4::new(1.05, 0.98, 0.9, 1.0),
    ));
    let boards = 13;
    for k in 0..boards {
        let x = -1.4 + 2.8 * (k as f32 + 0.5) / boards as f32;
        let hgt = top_y - 0.04 + rng.range(-0.03, 0.0);
        wood.merge(&tint(
            beam(
                Vec3::new(x, 0.0, counter_z + 0.24),
                Vec3::new(x + rng.range(-0.01, 0.01), hgt, counter_z + 0.24),
                0.2,
                0.025,
                0.006,
            ),
            weathered * rng.range(0.75, 1.0),
        ));
    }
    // Back shelf.
    for y in [0.7f32, 1.35] {
        wood.merge(&tint(
            moved(
                bevel_box(Vec3::new(3.0, 0.04, 0.38), 0.01, 0),
                Mat4::from_translation(Vec3::new(0.0, y, -hd + 0.2)),
            ),
            weathered,
        ));
    }
    // Crates and barrels around the stall.
    let mut iron = Mesh::default();
    for (pos, size, yaw) in [
        (Vec3::new(-2.25, 0.0, 0.7), Vec3::new(0.7, 0.55, 0.55), 0.3),
        (
            Vec3::new(-2.2, 0.55, 0.65),
            Vec3::new(0.55, 0.42, 0.45),
            0.1,
        ),
        (
            Vec3::new(-2.5, 0.0, -0.35),
            Vec3::new(0.62, 0.62, 0.62),
            -0.2,
        ),
        (Vec3::new(2.05, 0.0, -1.5), Vec3::new(0.8, 0.5, 0.6), 0.5),
    ] {
        let c = crate_mesh(size, &mut rng);
        wood.merge(&moved(
            c,
            Mat4::from_rotation_translation(
                Quat::from_rotation_y(yaw),
                pos + Vec3::Y * (size.y * 0.5),
            ),
        ));
    }
    for (pos, hgt, r) in [
        (Vec3::new(2.2, 0.0, 0.65), 0.95, 0.3),
        (Vec3::new(2.45, 0.0, 0.05), 0.8, 0.27),
    ] {
        let (w, i) = barrel_mesh(hgt, r);
        wood.merge(&tint(
            moved(w, Mat4::from_translation(pos)),
            Vec4::new(1.0, 0.82, 0.68, 1.0),
        ));
        iron.merge(&moved(i, Mat4::from_translation(pos)));
    }
    spawn_mesh(world, wood, mats.wood, at, true);

    // Canvas awning: sags between the beams, overhangs the front, and a scalloped valance.
    let (nu, nv) = (36usize, 26usize);
    let mut cloth = Mesh::default();
    let width = 2.0 * hw + 0.5;
    for j in 0..=nv {
        let v = j as f32 / nv as f32; // 0 = back, 1 = front edge
        for i in 0..=nu {
            let u = i as f32 / nu as f32;
            let x = (u - 0.5) * width;
            let z = -hd - 0.12 + v * (2.0 * hd + 0.45);
            let y_line = back_h + 0.1 + (front_h + 0.04 - back_h - 0.1) * v;
            let sag = 0.13 * (PI * u).sin().powf(0.8) * (PI * v.min(0.85) / 0.85).sin();
            let wrinkle = 0.012 * ((u * 23.0 + v * 4.0).sin() * (v * 11.0).cos())
                + 0.01 * noise::n2(u * 6.0, v * 6.0, 400);
            let p = Vec3::new(x, y_line - sag + wrinkle, z);
            cloth
                .vertices
                .push(vtx(p, Vec3::Y, Vec2::new(u, v * 0.82), Vec4::ONE));
        }
    }
    let row = (nu + 1) as u32;
    for j in 0..nv as u32 {
        for i in 0..nu as u32 {
            let a = j * row + i;
            cloth
                .indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    cloth.compute_normals();
    for v in &mut cloth.vertices {
        if v.normal[1] < 0.0 {
            v.normal = (-Vec3::from(v.normal)).to_array();
        }
    }
    fix_winding(&mut cloth);
    // Valance hanging from the front edge.
    let base = cloth.vertices.len() as u32;
    let front_z = hd + 0.33;
    for j in 0..=4 {
        let v = j as f32 / 4.0;
        for i in 0..=nu {
            let u = i as f32 / nu as f32;
            let x = (u - 0.5) * width;
            let y = front_h + 0.04 - v * 0.3;
            let p = Vec3::new(x, y, front_z + 0.02 * (u * 40.0).sin() * v);
            cloth
                .vertices
                .push(vtx(p, Vec3::Z, Vec2::new(u, 0.82 + 0.18 * v), Vec4::ONE));
        }
    }
    for j in 0..4u32 {
        for i in 0..nu as u32 {
            let a = base + j * row + i;
            cloth
                .indices
                .extend_from_slice(&[a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    spawn_mesh(world, cloth, mats.canvas, at, true);

    // Goods: crystals, pots and sacks.
    let mut crystals = Mesh::default();
    for k in 0..7 {
        let a = k as f32 * 2.3;
        let lean = Quat::from_euler(EulerRot::YXZ, a, rng.range(0.1, 0.5), 0.0);
        let len = rng.range(0.12, 0.3);
        crystals.merge(&moved(
            crystal(len, len * 0.18),
            Mat4::from_rotation_translation(
                lean,
                Vec3::new(-0.75, top_y + 0.035, counter_z - 0.05)
                    + Vec3::new(a.cos(), 0.0, a.sin()) * 0.05,
            ),
        ));
    }
    for k in 0..4 {
        let len = rng.range(0.1, 0.18);
        crystals.merge(&moved(
            crystal(len, len * 0.2),
            Mat4::from_rotation_translation(
                Quat::from_euler(EulerRot::YXZ, k as f32 * 1.7, 0.3, 0.0),
                Vec3::new(0.9 + k as f32 * 0.06, 1.37, -hd + 0.2),
            ),
        ));
    }
    spawn_mesh(world, crystals, mats.crystal, at, true);
    let mut pots = Mesh::default();
    for (x, y, z, hgt, r, color) in [
        (
            0.1f32,
            top_y + 0.035,
            counter_z - 0.1,
            0.26,
            0.11,
            hex(0x8a4a2a),
        ),
        (
            0.38,
            top_y + 0.035,
            counter_z - 0.02,
            0.18,
            0.08,
            hex(0x3d5a5e),
        ),
        (-0.3, 1.37, -hd + 0.2, 0.22, 0.09, hex(0x9a7a50)),
        (-0.05, 1.37, -hd + 0.18, 0.3, 0.1, hex(0x6a3424)),
        (0.4, 0.72, -hd + 0.2, 0.24, 0.12, hex(0x8a4a2a)),
    ] {
        let prof = [
            (0.0, 0.0),
            (r * 0.6, 0.0),
            (r, hgt * 0.35),
            (r * 0.9, hgt * 0.7),
            (r * 0.45, hgt * 0.85),
            (r * 0.55, hgt),
            (r * 0.42, hgt),
        ];
        pots.merge(&tint(
            moved(
                lathe(&prof, 28, r),
                Mat4::from_translation(Vec3::new(x, y, z)),
            ),
            rgba(color),
        ));
    }
    spawn_mesh(world, pots, mats.pottery, at, true);
    let mut sacks = Mesh::default();
    for (k, (x, z, s)) in [
        (-1.9f32, -1.25f32, 0.32f32),
        (-1.45, -1.35, 0.27),
        (1.4, 1.3, 0.3),
    ]
    .into_iter()
    .enumerate()
    {
        sacks.merge(&moved(
            sack_mesh(Vec3::new(s, s * 1.2, s), 500 + k as u32),
            Mat4::from_translation(Vec3::new(x, s * 0.7, z)),
        ));
    }
    spawn_mesh(world, sacks, mats.burlap, at, true);

    // Lantern under the front beam, with its light.
    let lantern_at = Vec3::new(-0.95, front_h - 0.62, hd + 0.02);
    let mut lantern_iron = Mesh::default();
    lantern_iron.merge(&tube(
        lantern_at + Vec3::Y * 0.3,
        Vec3::new(lantern_at.x, front_h - 0.04, lantern_at.z),
        0.006,
        6,
    ));
    lantern_iron.merge(&moved(
        lathe(&[(0.0, 0.26), (0.1, 0.2), (0.085, 0.18)], 16, 0.1),
        Mat4::from_translation(lantern_at),
    ));
    lantern_iron.merge(&moved(
        lathe(
            &[(0.0, -0.01), (0.08, -0.01), (0.08, 0.02), (0.0, 0.02)],
            16,
            0.08,
        ),
        Mat4::from_translation(lantern_at),
    ));
    for k in 0..4 {
        let a = k as f32 * FRAC_PI_2 + FRAC_PI_4;
        let o = Vec3::new(a.cos() * 0.075, 0.0, a.sin() * 0.075);
        lantern_iron.merge(&tube(
            lantern_at + o,
            lantern_at + o + Vec3::Y * 0.19,
            0.006,
            5,
        ));
    }
    iron.merge(&lantern_iron);
    spawn_mesh(world, iron, mats.iron, at, true);
    let glass = moved(
        lathe(
            &[
                (0.0, 0.02),
                (0.06, 0.03),
                (0.068, 0.1),
                (0.06, 0.18),
                (0.0, 0.19),
            ],
            16,
            0.06,
        ),
        Mat4::from_translation(lantern_at),
    );
    spawn_mesh(world, glass, mats.lamp_glass, at, false);
    let light_at = at.transform_point((lantern_at + Vec3::Y * 0.1).as_dvec3());
    world.spawn((
        PointLight {
            color: [1.0, 0.6, 0.3],
            intensity_lm: 380.0,
            range: 9.0,
            radius: 0.05,
        },
        Transform::from_translation(light_at),
    ));
}

// ============================================================================ the hab pod

fn spawn_hab(world: &mut World, mats: &Materials) {
    let ground = height(HAB.x, HAB.y, 0.03);
    let at = Transform::from_xyz(HAB.x as f64, ground as f64, HAB.y as f64)
        .with_rotation(Quat::from_rotation_y(0.55));
    let (len, r) = (4.4f32, 1.05f32);
    let mut hull = Mesh::default();
    // Body along x: a lathe around y, laid on its side; rounded end caps.
    let mut prof = vec![(0.0, -len * 0.5 - r * 0.45)];
    for k in 1..=8 {
        let a = k as f32 / 8.0 * FRAC_PI_2;
        prof.push((r * a.sin(), -len * 0.5 - r * 0.45 * a.cos()));
    }
    for k in 0..=10 {
        prof.push((r, -len * 0.5 + len * k as f32 / 10.0));
    }
    for k in (0..8).rev() {
        let a = k as f32 / 8.0 * FRAC_PI_2;
        prof.push((r * a.sin(), len * 0.5 + r * 0.45 * a.cos()));
    }
    let body = lathe(&prof, 48, r);
    let lay = Mat4::from_rotation_translation(
        Quat::from_rotation_z(FRAC_PI_2),
        Vec3::new(0.0, r + 0.35, 0.0),
    );
    let white = Vec4::new(0.17, 0.21, 0.2, 1.0);
    hull.merge(&tint(moved(body, lay), white));
    // Ribs and an orange band.
    for k in 0..5 {
        let y = -len * 0.5 + 0.3 + (len - 0.6) * k as f32 / 4.0;
        let rib = lathe(
            &[
                (r, y - 0.07),
                (r + 0.05, y - 0.05),
                (r + 0.05, y + 0.05),
                (r, y + 0.07),
            ],
            48,
            r,
        );
        hull.merge(&tint(moved(rib, lay), Vec4::new(0.24, 0.24, 0.24, 1.0)));
    }
    let band = lathe(&[(r + 0.004, 0.6), (r + 0.004, 1.4)], 48, r);
    hull.merge(&tint(moved(band, lay), rgba(hex(0xd8692a))));
    // Hatch on the side facing the camera, and skids.
    hull.merge(&tint(
        moved(
            bevel_box(Vec3::new(0.9, 1.5, 0.12), 0.04, 1),
            Mat4::from_translation(Vec3::new(-0.6, r + 0.35, r - 0.02)),
        ),
        Vec4::new(0.3, 0.3, 0.29, 1.0),
    ));
    for x in [-1.8f32, 1.8] {
        hull.merge(&tint(
            moved(
                bevel_box(Vec3::new(0.25, 0.45, 2.2), 0.03, 2),
                Mat4::from_translation(Vec3::new(x, 0.12, 0.0)),
            ),
            Vec4::new(0.45, 0.45, 0.46, 1.0),
        ));
    }
    // Antenna mast with a dish.
    let mast_base = Vec3::new(1.6, r * 2.0 + 0.3, 0.0);
    hull.merge(&tint(
        tube(mast_base, mast_base + Vec3::Y * 3.2, 0.04, 8),
        Vec4::new(0.6, 0.6, 0.6, 1.0),
    ));
    let dish = lathe(
        &[
            (0.0, 0.0),
            (0.25, 0.03),
            (0.5, 0.12),
            (0.55, 0.16),
            (0.53, 0.165),
            (0.0, 0.03),
        ],
        32,
        0.5,
    );
    hull.merge(&moved(
        dish,
        Mat4::from_rotation_translation(
            Quat::from_euler(EulerRot::YXZ, -0.9, 1.1, 0.0),
            mast_base + Vec3::new(0.0, 2.4, 0.15),
        ),
    ));
    // Dust and grime build up toward the ground.
    for v in &mut hull.vertices {
        let k = sstep(0.2, 1.7, v.position[1]);
        let dust = Vec3::new(0.62, 0.48, 0.34);
        let c = Vec3::new(v.color[0], v.color[1], v.color[2]);
        v.color = rgba(mix3(c * dust, c, 0.35 + 0.65 * k)).to_array();
    }
    spawn_mesh(world, hull, mats.metal, at, true);
    let window = moved(
        bevel_box(Vec3::new(0.5, 0.32, 0.05), 0.04, 0),
        Mat4::from_translation(Vec3::new(0.7, r + 0.75, r - 0.0)),
    );
    spawn_mesh(world, window, mats.window, at, false);
    // A water tank beside it.
    let tank = lathe(
        &[
            (0.0, 0.0),
            (0.55, 0.0),
            (0.6, 0.08),
            (0.6, 1.5),
            (0.5, 1.62),
            (0.0, 1.66),
        ],
        32,
        0.6,
    );
    let tank = tint(
        moved(tank, Mat4::from_translation(Vec3::new(-3.3, 0.0, 1.6))),
        Vec4::new(0.3, 0.22, 0.16, 1.0),
    );
    spawn_mesh(world, tank, mats.metal, at, true);
    let tip = at.transform_point((mast_base + Vec3::Y * 3.25).as_dvec3());
    let bulb = add_mesh(
        world,
        lathe(&[(0.0, -0.05), (0.05, 0.0), (0.0, 0.05)], 12, 0.05),
    );
    world.spawn((
        Transform::from_translation(tip),
        MeshRenderer::new(bulb, mats.beacon_green).without_shadows(),
        Beacon {
            period: 2.2,
            phase: 0.3,
            on_strength: 4_000.0,
        },
    ));

    // A small wind turbine behind the hab: lattice-free mast, nacelle and three blades.
    let base = Vec3::new(2.6, 0.0, -2.8);
    let top = base + Vec3::Y * 6.4;
    let mut turbine = Mesh::default();
    turbine.merge(&tube(base, top, 0.09, 10));
    turbine.merge(&tube(
        base + Vec3::new(0.0, 0.0, 0.0),
        base + Vec3::new(1.4, -0.2, 0.9),
        0.025,
        5,
    ));
    turbine.merge(&tube(base, base + Vec3::new(-1.3, -0.2, 1.0), 0.025, 5));
    turbine.merge(&moved(
        bevel_box(Vec3::new(0.32, 0.3, 0.9), 0.08, 2),
        Mat4::from_translation(top + Vec3::new(0.0, 0.1, 0.1)),
    ));
    let hub = top + Vec3::new(0.0, 0.1, 0.62);
    turbine.merge(&moved(
        lathe(
            &[(0.0, -0.05), (0.13, 0.0), (0.12, 0.12), (0.0, 0.28)],
            12,
            0.13,
        ),
        Mat4::from_rotation_translation(Quat::from_rotation_x(FRAC_PI_2), hub),
    ));
    for k in 0..3 {
        let a = k as f32 * TAU / 3.0 + 0.4;
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        let blade = bevel_box(Vec3::new(0.15, 1.8, 0.03), 0.012, 1);
        let twist = Quat::from_rotation_arc(Vec3::Y, dir) * Quat::from_rotation_y(0.35);
        turbine.merge(&moved(
            blade,
            Mat4::from_rotation_translation(twist, hub + dir * 1.0),
        ));
    }
    spawn_mesh(
        world,
        tint(turbine, Vec4::new(0.55, 0.55, 0.53, 1.0)),
        mats.metal,
        at,
        true,
    );
}

/// Way markers along the path past the stall: weathered posts with a strip of red cloth.
fn spawn_way_markers(world: &mut World, mats: &Materials) {
    let mut wood = Mesh::default();
    let mut cloth = Mesh::default();
    let mut rng = Rng::new(31);
    let mut placed = 0;
    let mut walked = 0.0;
    for w in PATH.windows(2) {
        let (a, b) = (w[0], w[1]);
        let len = a.distance(b);
        let side = Vec2::new(b.y - a.y, a.x - b.x) / len;
        while walked < len {
            let p = a + (b - a) * (walked / len);
            if p.y < -12.0 {
                let q = p + side
                    * (1.5 + rng.range(-0.15, 0.15))
                    * if placed % 2 == 0 { 1.0 } else { -1.0 };
                let g = height(q.x, q.y, 0.03);
                let lean = Vec3::new(rng.range(-0.06, 0.06), 0.0, rng.range(-0.06, 0.06));
                let foot = Vec3::new(q.x, g - 0.25, q.y);
                let head = Vec3::new(q.x, g + 1.25, q.y) + lean;
                wood.merge(&tint(
                    beam(foot, head, 0.09, 0.09, 0.012),
                    Vec4::new(0.8, 0.72, 0.62, 1.0),
                ));
                // A strip of cloth tied below the top, hanging and lifting a little in the wind.
                let tie = head - Vec3::Y * 0.12;
                let flow = Vec3::new(0.85, -0.25, -0.35).normalize();
                let base = cloth.vertices.len() as u32;
                for k in 0..=4 {
                    let t = k as f32 / 4.0;
                    let c = tie
                        + flow * (0.4 * t)
                        + Vec3::new(0.0, -0.08 * t * t, 0.03 * (t * 7.0).sin());
                    let half = Vec3::Y * (0.045 * (1.0 - 0.4 * t));
                    cloth.vertices.push(vtx(
                        c + half,
                        Vec3::Z,
                        Vec2::new(0.01 + 0.04 * t, 0.3),
                        Vec4::ONE,
                    ));
                    cloth.vertices.push(vtx(
                        c - half,
                        Vec3::Z,
                        Vec2::new(0.01 + 0.04 * t, 0.32),
                        Vec4::ONE,
                    ));
                }
                for k in 0..4u32 {
                    let i = base + k * 2;
                    cloth
                        .indices
                        .extend_from_slice(&[i, i + 1, i + 2, i + 1, i + 3, i + 2]);
                }
                placed += 1;
            }
            walked += 7.5;
        }
        walked -= len;
    }
    spawn_mesh(world, wood, mats.wood, Transform::default(), true);
    cloth.compute_normals();
    spawn_mesh(world, cloth, mats.canvas, Transform::default(), false);
}

// ============================================================================ lookout

fn spawn_lookout(world: &mut World, mats: &Materials) {
    let ground = height(LOOKOUT.x, LOOKOUT.y, 0.2);
    let at = Transform::from_xyz(LOOKOUT.x as f64, ground as f64, LOOKOUT.y as f64)
        .with_rotation(Quat::from_rotation_y(0.3));
    let mut wood = Mesh::default();
    let (b, t, deck) = (1.6f32, 1.15f32, 7.0f32);
    let legs = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    for (sx, sz) in legs {
        wood.merge(&beam(
            Vec3::new(sx * b, -0.5, sz * b),
            Vec3::new(sx * t, deck + 2.6, sz * t),
            0.24,
            0.24,
            0.03,
        ));
    }
    // Cross braces on every side, two storeys.
    for k in 0..4 {
        let (a0, a1) = (legs[k], legs[(k + 1) % 4]);
        for (y0, y1) in [(0.3f32, 3.6f32), (3.6, deck)] {
            let lerp_leg = |l: (f32, f32), y: f32| {
                let s = b + (t - b) * (y + 0.5) / (deck + 3.1);
                Vec3::new(l.0 * s, y, l.1 * s)
            };
            wood.merge(&beam(lerp_leg(a0, y0), lerp_leg(a1, y1), 0.14, 0.1, 0.02));
            wood.merge(&beam(lerp_leg(a1, y0), lerp_leg(a0, y1), 0.14, 0.1, 0.02));
        }
    }
    // Deck, railing, roof.
    wood.merge(&moved(
        bevel_box(Vec3::new(3.0, 0.18, 3.0), 0.03, 0),
        Mat4::from_translation(Vec3::Y * deck),
    ));
    for k in 0..4 {
        let (a0, a1) = (legs[k], legs[(k + 1) % 4]);
        let s = 1.42;
        wood.merge(&beam(
            Vec3::new(a0.0 * s, deck + 1.0, a0.1 * s),
            Vec3::new(a1.0 * s, deck + 1.0, a1.1 * s),
            0.1,
            0.1,
            0.02,
        ));
    }
    let roof = lathe(
        &[
            (0.0, deck + 2.5),
            (2.4, deck + 2.5),
            (2.35, deck + 2.62),
            (0.0, deck + 3.7),
        ],
        4,
        1.0,
    );
    wood.merge(&moved(roof, Mat4::from_rotation_y(FRAC_PI_4)));
    wood.merge(&tube(
        Vec3::Y * (deck + 3.6),
        Vec3::Y * (deck + 5.6),
        0.05,
        6,
    ));
    spawn_mesh(
        world,
        tint(wood, Vec4::new(0.85, 0.78, 0.7, 1.0)),
        mats.wood,
        at,
        true,
    );
    // Pennant.
    let flag = Mesh::new(
        vec![
            vtx(
                Vec3::new(0.0, deck + 5.5, 0.0),
                Vec3::Z,
                Vec2::new(0.1, 0.1),
                Vec4::ONE,
            ),
            vtx(
                Vec3::new(1.4, deck + 5.2, 0.15),
                Vec3::Z,
                Vec2::new(0.4, 0.15),
                Vec4::ONE,
            ),
            vtx(
                Vec3::new(0.0, deck + 4.9, 0.0),
                Vec3::Z,
                Vec2::new(0.1, 0.2),
                Vec4::ONE,
            ),
        ],
        vec![0, 2, 1],
    );
    spawn_mesh(world, flag, mats.canvas, at, false);
    let lamp_at = at.transform_point(DVec3::new(0.0, deck as f64 + 1.6, 0.0));
    let bulb = add_mesh(
        world,
        lathe(
            &[(0.0, -0.18), (0.16, -0.1), (0.16, 0.1), (0.0, 0.18)],
            12,
            0.16,
        ),
    );
    world.spawn((
        Transform::from_translation(lamp_at),
        MeshRenderer::new(bulb, mats.lamp_glass).without_shadows(),
    ));
    world.spawn((
        PointLight {
            color: [1.0, 0.62, 0.32],
            intensity_lm: 4000.0,
            range: 22.0,
            radius: 0.15,
        },
        Transform::from_translation(lamp_at),
    ));
}

// ============================================================================ relay tower

fn spawn_tower(world: &mut World, mats: &Materials) {
    let ground = height(TOWER.x, TOWER.y, 1.0);
    let at = Transform::from_xyz(TOWER.x as f64, ground as f64, TOWER.y as f64)
        .with_rotation(Quat::from_rotation_y(0.2));
    let hgt = 46.0f32;
    let (r0, r1) = (3.2f32, 1.0f32);
    let corner = |k: usize, y: f32| {
        let a = k as f32 * TAU / 3.0;
        let r = r0 + (r1 - r0) * y / hgt;
        Vec3::new(a.cos() * r, y, a.sin() * r)
    };
    let mut steel = Mesh::default();
    for k in 0..3 {
        steel.merge(&tube(corner(k, -1.0), corner(k, hgt), 0.32, 6));
    }
    for s in 0..12 {
        let y = s as f32 * hgt / 12.0;
        for k in 0..3 {
            steel.merge(&tube(corner(k, y), corner((k + 1) % 3, y), 0.16, 4));
        }
    }
    // Platform, dishes and the top mast.
    steel.merge(&moved(
        lathe(&[(0.0, 0.0), (2.4, 0.0), (2.4, 0.3), (0.0, 0.3)], 12, 2.0),
        Mat4::from_translation(Vec3::Y * (hgt * 0.72)),
    ));
    for (y, yaw, size) in [(hgt * 0.75, 0.6f32, 2.2f32), (hgt * 0.62, 2.8, 1.6)] {
        let dish = lathe(
            &[
                (0.0, 0.0),
                (size * 0.5, size * 0.06),
                (size, size * 0.25),
                (size * 0.97, size * 0.26),
                (0.0, size * 0.06),
            ],
            24,
            size,
        );
        steel.merge(&moved(
            dish,
            Mat4::from_rotation_translation(
                Quat::from_euler(EulerRot::YXZ, yaw, FRAC_PI_2, 0.0),
                Vec3::new(yaw.cos() * 1.8, y, -yaw.sin() * 1.8),
            ),
        ));
    }
    steel.merge(&tube(Vec3::Y * hgt, Vec3::Y * (hgt + 9.0), 0.22, 6));
    // A small equipment shed at the foot.
    steel.merge(&moved(
        bevel_box(Vec3::new(5.0, 3.0, 3.5), 0.1, 0),
        Mat4::from_translation(Vec3::new(7.0, 1.3, 2.0)),
    ));
    spawn_mesh(
        world,
        tint(steel, Vec4::new(0.35, 0.34, 0.33, 1.0)),
        mats.iron,
        at,
        true,
    );
    // Lattice panels between the legs (blended, so they fade to a haze at a distance).
    let mut panels = Mesh::default();
    for k in 0..3 {
        let (a, b) = (k, (k + 1) % 3);
        let base = panels.vertices.len() as u32;
        let n = (corner(a, 0.0) + corner(b, 0.0)).normalize() * Vec3::new(1.0, 0.0, 1.0);
        for (p, uv) in [
            (corner(a, 0.0), Vec2::new(0.0, hgt / 4.0)),
            (corner(b, 0.0), Vec2::new(1.0, hgt / 4.0)),
            (corner(b, hgt), Vec2::new(1.0, 0.0)),
            (corner(a, hgt), Vec2::new(0.0, 0.0)),
        ] {
            panels.vertices.push(vtx(p, n, uv, Vec4::ONE));
        }
        panels
            .indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    fix_winding(&mut panels);
    // Blended, so drawn live over the replayed scene (see `PostFx`).
    let lattice = spawn_mesh(world, panels, mats.lattice, at, false);
    world.entity_mut(lattice).remove::<StaticBackdrop>();
    // Warning light at the top.
    let light_at = at.transform_point(DVec3::new(0.0, hgt as f64 + 9.3, 0.0));
    let bulb = add_mesh(
        world,
        lathe(&[(0.0, -0.9), (0.9, 0.0), (0.0, 0.9)], 12, 0.9),
    );
    world.spawn((
        Transform::from_translation(light_at),
        MeshRenderer::new(bulb, mats.beacon_red).without_shadows(),
        Beacon {
            period: 1.6,
            phase: 0.0,
            on_strength: 60_000.0,
        },
    ));
}

// ============================================================================ moon

fn spawn_moon(world: &mut World, tex: &Textures) {
    // Low over the mountains, right of centre; a quarter lit by the low sun.
    let (az, el) = (29f32.to_radians(), 17.5f32.to_radians());
    let dir = Vec3::new(az.sin() * el.cos(), el.sin(), -az.cos() * el.cos());
    let dist = 2.0e6f32;
    let radius = dist * 1.7f32.to_radians();
    let black = add_image(world, Image::solid([0, 0, 0, 255], false));
    let mat = add_mat(
        world,
        StandardMaterial {
            base_color: [1.0; 4],
            perceptual_roughness: 1.0,
            reflectance: 0.0,
            base_color_texture: Some(tex.moon.albedo),
            occlusion_texture: Some(black),
            occlusion_strength: 1.0,
            receives_wetness: false,
            ..Default::default()
        },
    );
    let m = add_mesh(
        world,
        Shape::Sphere {
            radius,
            segments: 96,
        }
        .mesh(),
    );
    world.spawn((
        Transform::from_translation(EYE + (dir * dist).as_dvec3()).with_rotation(Quat::from_euler(
            EulerRot::YXZ,
            0.6,
            0.3,
            0.2,
        )),
        MeshRenderer::new(m, mat).without_shadows(),
        StaticBackdrop,
    ));
}

// ============================================================================ beacons

/// A blinking light: its material's emission pulses.
#[derive(Component)]
struct Beacon {
    period: f32,
    phase: f32,
    on_strength: f32,
}

fn blink_beacons(
    time: Res<Time>,
    beacons: Query<(&Beacon, &MeshRenderer)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let t = time.elapsed_secs();
    for (b, r) in &beacons {
        let MaterialRef::Handle(h) = &r.material else {
            continue;
        };
        let x = ((t / b.period + b.phase).fract() - 0.15).abs();
        // Mostly on, with a short dark beat (stills catch it lit).
        let k = 0.12 + 0.88 * sstep(0.62, 0.5, x);
        if let Some(m) = materials.get_mut(*h) {
            let s = b.on_strength * k;
            if (m.emissive_strength - s).abs() > s * 0.02 + 1.0 {
                m.emissive_strength = s;
            }
        }
    }
}

// ============================================================================ post pass

/// Marks what [`PostFx`] may hide while it replays a captured frame: every static mesh, and
/// the sun (whose shadows are switched off meanwhile).
#[derive(Component)]
struct StaticBackdrop;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum CacheState {
    /// Everything is drawn every frame.
    #[default]
    Live,
    /// Capturing last frame's finished scene this frame (and replaying it).
    Capture,
    /// Replaying the captured frame; static meshes and sun shadows are off.
    Cached,
}

/// What a captured frame depends on.
#[derive(Clone, PartialEq)]
struct CacheKey {
    view_proj: [f32; 16],
    origin: [f64; 3],
    size: (u32, u32),
    exposure: f32,
    sun: [f32; 3],
    settings: RenderSettings,
}

struct CacheTargets {
    size: (u32, u32),
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    group: wgpu::BindGroup,
}

/// FXAA (anti-aliasing) and screen-space ambient occlusion over the HDR scene, drawn first in
/// the transparent phase from the renderer's scene copy; plus a static-scene cache.
///
/// Nothing in the backdrop moves but the beacons, and a software rasterizer spends seconds
/// on every frame of it. So once the view (camera, size, exposure, sun, settings) has held
/// still for a few frames, the finished scene (after this pass) is captured with its depth,
/// and from then on replayed in the opaque phase: the static meshes are hidden, the sun's
/// shadows are off, and the sky pass finds every pixel already covered. Anything not marked
/// [`StaticBackdrop`] (the beacons, the tower's blended lattice, UI) is still drawn live,
/// depth tested against the replayed depth. Any change to the view goes back to live
/// rendering.
#[derive(Default)]
struct PostFx {
    layout: Option<wgpu::BindGroupLayout>,
    sampler: Option<wgpu::Sampler>,
    live: Option<wgpu::RenderPipeline>,
    capture: Option<wgpu::RenderPipeline>,
    replay: Option<wgpu::RenderPipeline>,
    replay_layout: Option<wgpu::BindGroupLayout>,
    targets: Option<CacheTargets>,
    state: CacheState,
    key: Option<CacheKey>,
    stable: u32,
}

const POST_WGSL: &str = r#"
@group(1) @binding(0) var scene: texture_2d<f32>;
@group(1) @binding(1) var depth: texture_depth_2d;
@group(1) @binding(2) var lin: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

fn luma(c: vec3<f32>) -> f32 {
    let l = luminance(c);
    return sqrt(l / (1.0 + l));
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(scene, lin, uv, 0.0).rgb;
}

// View-space position from reverse-Z depth.
fn view_pos(px: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(depth));
    let q = clamp(px, vec2<i32>(0), size - 1);
    let d = textureLoad(depth, q, 0);
    let uv = (vec2<f32>(q) + 0.5) / vec2<f32>(size);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, max(d, 1e-7), 1.0);
    let v = frame.inv_proj * ndc;
    return v.xyz / v.w;
}

fn fxaa(uv: vec2<f32>, rcp: vec2<f32>) -> vec3<f32> {
    let c = tap(uv);
    let lm = luma(c);
    let ln = luma(tap(uv + vec2(0.0, -rcp.y)));
    let ls = luma(tap(uv + vec2(0.0, rcp.y)));
    let le = luma(tap(uv + vec2(rcp.x, 0.0)));
    let lw = luma(tap(uv + vec2(-rcp.x, 0.0)));
    let lo = min(lm, min(min(ln, ls), min(le, lw)));
    let hi = max(lm, max(max(ln, ls), max(le, lw)));
    let range = hi - lo;
    if (range < max(0.03, hi * 0.1)) {
        return c;
    }
    let lne = luma(tap(uv + vec2(rcp.x, -rcp.y)));
    let lnw = luma(tap(uv + vec2(-rcp.x, -rcp.y)));
    let lse = luma(tap(uv + vec2(rcp.x, rcp.y)));
    let lsw = luma(tap(uv + vec2(-rcp.x, rcp.y)));
    let horizontal = abs(ln + ls - 2.0 * lm) * 2.0 + abs(lne + lse - 2.0 * le) + abs(lnw + lsw - 2.0 * lw);
    let vertical = abs(le + lw - 2.0 * lm) * 2.0 + abs(lne + lnw - 2.0 * ln) + abs(lse + lsw - 2.0 * ls);
    let is_h = horizontal >= vertical;
    let l1 = select(lw, ln, is_h);
    let l2 = select(le, ls, is_h);
    let g1 = abs(l1 - lm);
    let g2 = abs(l2 - lm);
    var step_len = select(rcp.x, rcp.y, is_h);
    var l_edge: f32;
    var grad: f32;
    if (g1 >= g2) {
        step_len = -step_len;
        l_edge = (l1 + lm) * 0.5;
        grad = g1;
    } else {
        l_edge = (l2 + lm) * 0.5;
        grad = g2;
    }
    var edge_uv = uv;
    if (is_h) { edge_uv.y += step_len * 0.5; } else { edge_uv.x += step_len * 0.5; }
    let along = select(vec2(0.0, rcp.y), vec2(rcp.x, 0.0), is_h);
    var p1 = edge_uv - along;
    var p2 = edge_uv + along;
    var e1 = luma(tap(p1)) - l_edge;
    var e2 = luma(tap(p2)) - l_edge;
    var done1 = abs(e1) >= grad * 0.25;
    var done2 = abs(e2) >= grad * 0.25;
    var steps = array<f32, 10>(1.0, 1.0, 1.0, 1.5, 2.0, 2.0, 2.0, 4.0, 8.0, 8.0);
    for (var i = 0; i < 10; i++) {
        if (done1 && done2) { break; }
        if (!done1) { p1 -= along * steps[i]; e1 = luma(tap(p1)) - l_edge; done1 = abs(e1) >= grad * 0.25; }
        if (!done2) { p2 += along * steps[i]; e2 = luma(tap(p2)) - l_edge; done2 = abs(e2) >= grad * 0.25; }
    }
    let d1 = select(uv.y - p1.y, uv.x - p1.x, is_h);
    let d2 = select(p2.y - uv.y, p2.x - uv.x, is_h);
    let closest = min(d1, d2);
    let total = d1 + d2;
    let pixel_offset = -closest / total + 0.5;
    let lm_smaller = lm < l_edge;
    let correct = (select(e2, e1, d1 < d2) < 0.0) != lm_smaller;
    var final_offset = select(0.0, pixel_offset, correct);
    // Subpixel aliasing.
    let avg = (2.0 * (ln + ls + le + lw) + lne + lnw + lse + lsw) / 12.0;
    let sub1 = saturate(abs(avg - lm) / range);
    let sub2 = (-2.0 * sub1 + 3.0) * sub1 * sub1;
    final_offset = max(final_offset, sub2 * sub2 * 0.6);
    var out_uv = uv;
    if (is_h) { out_uv.y += final_offset * step_len; } else { out_uv.x += final_offset * step_len; }
    return tap(out_uv);
}

// Horizon-ish SSAO from the depth buffer: 12 taps on a spiral within a 0.6 m sphere.
fn ssao(px: vec2<i32>) -> f32 {
    let p = view_pos(px);
    let dist = -p.z;
    if (dist > 400.0) { return 1.0; }
    let pr = view_pos(px + vec2(1, 0));
    let pd = view_pos(px + vec2(0, 1));
    let n = normalize(cross(pd - p, pr - p));
    let radius_m = 0.55;
    let proj_scale = f32(textureDimensions(depth).y) * frame.proj[1][1] * 0.5;
    let radius_px = clamp(radius_m * proj_scale / dist, 2.0, 90.0);
    let jitter = hash12(vec2<f32>(px)) * TAU;
    var occ = 0.0;
    for (var i = 0; i < 12; i++) {
        let f = (f32(i) + 0.5) / 12.0;
        let a = jitter + f32(i) * 2.39996;
        let o = vec2<f32>(cos(a), sin(a)) * radius_px * f;
        let q = view_pos(px + vec2<i32>(o));
        let v = q - p;
        let vv = dot(v, v);
        let vn = dot(v, n);
        let falloff = saturate(1.0 - vv / (radius_m * radius_m * 4.0));
        occ += max(vn / sqrt(vv + 1e-4) - 0.12, 0.0) * falloff;
    }
    return saturate(1.0 - occ / 12.0 * 1.6);
}

// The sky's gradient shows faint steps: blur it vertically, over sky pixels only.
fn sky_smooth(px: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(scene));
    var sum = vec3<f32>(0.0);
    var w = 0.0;
    for (var i = -8; i <= 8; i++) {
        let q = clamp(px + vec2<i32>(0, i * 5), vec2<i32>(0), size - 1);
        if (textureLoad(depth, q, 0) <= 0.0) {
            let k = 1.0 - abs(f32(i)) / 9.0;
            sum += textureLoad(scene, q, 0).rgb * k;
            w += k;
        }
    }
    return sum / max(w, 1e-4);
}

fn post(in: FullscreenOut) -> vec3<f32> {
    let px = vec2<i32>(in.position.xy);
    if (textureLoad(depth, px, 0) <= 0.0) {
        return sky_smooth(px);
    }
    let rcp = 1.0 / vec2<f32>(textureDimensions(scene));
    let c = fxaa(in.uv, rcp);
    let ao = ssao(vec2<i32>(in.position.xy));
    return c * mix(1.0, ao, 0.85);
}

@fragment
fn fs(in: FullscreenOut) -> @location(0) vec4<f32> {
    return vec4<f32>(post(in), 1.0);
}

struct Captured {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
}

@fragment
fn fs_capture(in: FullscreenOut) -> Captured {
    var o: Captured;
    o.color = vec4<f32>(post(in), 1.0);
    o.depth = textureLoad(depth, vec2<i32>(in.position.xy), 0);
    return o;
}
"#;

const REPLAY_WGSL: &str = r#"
@group(1) @binding(0) var cached_color: texture_2d<f32>;
@group(1) @binding(1) var cached_depth: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> FullscreenOut {
    return fullscreen_vertex(i, 0.0);
}

struct Replayed {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@fragment
fn fs(in: FullscreenOut) -> Replayed {
    let px = vec2<i32>(in.position.xy);
    var o: Replayed;
    o.color = textureLoad(cached_color, px, 0);
    // Where the sky was, stay just in front of the far plane (0) so the sky pass skips it.
    o.depth = max(textureLoad(cached_depth, px, 0).r, 1e-10);
    return o;
}
"#;

fn texture_entry(binding: u32, sample_type: wgpu::TextureSampleType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

/// Show or hide everything marked [`StaticBackdrop`] (and switch the sun's shadows with it).
fn show_static(world: &mut World, show: bool) {
    let mut q = world.query_filtered::<&mut MeshRenderer, With<StaticBackdrop>>();
    for mut r in q.iter_mut(world) {
        r.visible = show;
    }
    let mut q = world.query_filtered::<&mut DirectionalLight, With<StaticBackdrop>>();
    for mut l in q.iter_mut(world) {
        l.cast_shadows = show;
    }
}

impl PostFx {
    fn pipeline(
        gpu: &GpuShared,
        label: &str,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::BindGroupLayout,
        fs: &str,
        targets: &[Option<wgpu::ColorTargetState>],
        depth: Option<wgpu::DepthStencilState>,
    ) -> wgpu::RenderPipeline {
        let device = &gpu.device;
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: &[Some(&gpu.frame_layout), Some(layout)],
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: depth,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets,
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// Bind group over the renderer's scene copies.
    fn scene_group(&self, ctx: &FrameContext) -> Option<wgpu::BindGroup> {
        let (layout, sampler) = (self.layout.as_ref()?, self.sampler.as_ref()?);
        Some(
            ctx.gpu
                .device
                .create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("gallery postfx"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_color_copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(ctx.scene_depth_copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                }),
        )
    }

    /// The capture targets, (re)created at `size`.
    fn targets_for(&mut self, gpu: &GpuShared, size: (u32, u32)) -> Option<&CacheTargets> {
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            let layout = self.replay_layout.as_ref()?;
            let texture = |label: &str, format: wgpu::TextureFormat| {
                gpu.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: size.0.max(1),
                            height: size.1.max(1),
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            let color = texture("gallery cache color", gpu.formats.hdr);
            let depth = texture("gallery cache depth", wgpu::TextureFormat::R32Float);
            let group = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gallery cache"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&color),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&depth),
                    },
                ],
            });
            self.targets = Some(CacheTargets {
                size,
                color,
                depth,
                group,
            });
        }
        self.targets.as_ref()
    }
}

impl RenderFeature for PostFx {
    fn name(&self) -> &str {
        "gallery_postfx"
    }

    fn order(&self) -> i32 {
        -1000
    }

    fn init(&mut self, gpu: &GpuShared) {
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gallery postfx"),
                entries: &[
                    texture_entry(0, float),
                    texture_entry(1, wgpu::TextureSampleType::Depth),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let replay_layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gallery cache"),
                entries: &[
                    texture_entry(0, float),
                    texture_entry(1, wgpu::TextureSampleType::Float { filterable: false }),
                ],
            });
        let target = |format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let hdr = gpu.formats.hdr;
        let post = gpu.shader("gallery postfx", POST_WGSL, true);
        let replay = gpu.shader("gallery replay", REPLAY_WGSL, true);
        let no_depth_test = wgpu::DepthStencilState {
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            ..gpu.depth_state(false)
        };
        let write_depth = wgpu::DepthStencilState {
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            ..gpu.depth_state(true)
        };
        self.live = Some(Self::pipeline(
            gpu,
            "gallery postfx",
            &post,
            &layout,
            "fs",
            &[target(hdr)],
            Some(no_depth_test),
        ));
        self.capture = Some(Self::pipeline(
            gpu,
            "gallery capture",
            &post,
            &layout,
            "fs_capture",
            &[target(hdr), target(wgpu::TextureFormat::R32Float)],
            None,
        ));
        self.replay = Some(Self::pipeline(
            gpu,
            "gallery replay",
            &replay,
            &replay_layout,
            "fs",
            &[target(hdr)],
            Some(write_depth),
        ));
        self.sampler = Some(gpu.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gallery postfx"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        }));
        self.layout = Some(layout);
        self.replay_layout = Some(replay_layout);
    }

    fn prepare(
        &mut self,
        world: &mut World,
        _gpu: &GpuShared,
        view: &ViewData,
        settings: &RenderSettings,
    ) {
        let key = CacheKey {
            view_proj: view.view_proj.to_cols_array(),
            origin: view.origin.to_array(),
            size: (view.width, view.height),
            exposure: view.exposure,
            sun: view.sun_dir.to_array(),
            settings: settings.clone(),
        };
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.stable = 0;
            if self.state != CacheState::Live {
                self.state = CacheState::Live;
                show_static(world, true);
            }
            return;
        }
        self.stable += 1;
        match self.state {
            // Three still frames: the last one was drawn live, with shadows, from this view.
            CacheState::Live if self.stable >= 3 => {
                self.state = CacheState::Capture;
                show_static(world, false);
            }
            CacheState::Capture => self.state = CacheState::Cached,
            _ => {}
        }
    }

    fn compute(&mut self, encoder: &mut wgpu::CommandEncoder, ctx: &FrameContext) {
        if self.state != CacheState::Capture {
            return;
        }
        let Some(group) = self.scene_group(ctx) else {
            return;
        };
        let Some(targets) = self.targets_for(ctx.gpu, (ctx.view.width, ctx.view.height)) else {
            return;
        };
        let attachment = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gallery capture"),
            color_attachments: &[attachment(&targets.color), attachment(&targets.depth)],
            ..Default::default()
        });
        if let Some(p) = &self.capture {
            pass.set_pipeline(p);
            pass.set_bind_group(0, ctx.frame_group, &[]);
            pass.set_bind_group(1, &group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn needs_scene_copy(&self) -> bool {
        self.state == CacheState::Live
    }

    fn draw(&self, phase: RenderPhase, pass: &mut wgpu::RenderPass<'_>, ctx: &FrameContext) {
        match (phase, self.state) {
            (RenderPhase::Opaque, CacheState::Capture | CacheState::Cached) => {
                if let (Some(p), Some(t)) = (&self.replay, &self.targets) {
                    pass.set_pipeline(p);
                    pass.set_bind_group(1, &t.group, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            (RenderPhase::Transparent, CacheState::Live) => {
                if let (Some(p), Some(group)) = (&self.live, self.scene_group(ctx)) {
                    pass.set_pipeline(p);
                    pass.set_bind_group(1, &group, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            _ => {}
        }
    }
}
