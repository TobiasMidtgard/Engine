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
use engine_render::{Mesh, ViewData, wgpu};
use engine_ui::demo::art::mesh::{beam, bevel_box, box_uv_unshared, icosphere, lathe, tube};
use engine_ui::demo::art::stall::{Goods, StallMats, spawn_stall as build_stall};
use engine_ui::demo::art::textures::{
    brannoc_canvas_texel, dirt_texel, linen_texel, metal_texel, rock_texel, wood_texel,
};
use engine_ui::demo::art::{
    TexSet, Texel, add_image, add_mat, add_mesh, bake_set, fix_winding, hex, lamp, mix3, moved,
    noise, par_map, rgba, sat, sstep, textured, tint, vtx,
};
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

// ============================================================================ textures

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
            canvas: bake_set(world, 1024, 1024, 3.4, false, 1.0, brannoc_canvas_texel),
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

impl Materials {
    fn new(world: &mut World, t: &Textures) -> Self {
        let ground = |set: &TexSet, size: f32| StandardMaterial {
            reflectance: 0.4,
            ..textured(set, 1.0 / size)
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

fn spawn_stall(world: &mut World, mats: &Materials) {
    let ground = height(STALL.x, STALL.y, 0.03);
    let at = Transform::from_xyz(STALL.x as f64, ground as f64, STALL.y as f64)
        .with_rotation(Quat::from_rotation_y(STALL_YAW));
    // The trader's stall uses none of the other trades' materials; its counter runner and
    // scrolls are dyed linen, its scales and coins brass.
    let linen = bake_set(world, 512, 512, 0.5, true, 1.0, linen_texel);
    let cloth = add_mat(
        world,
        StandardMaterial {
            double_sided: true,
            subsurface: 0.45,
            ..textured(&linen, 2.0)
        },
    );
    let brass = add_mat(
        world,
        StandardMaterial {
            base_color: [0.42, 0.27, 0.08, 1.0],
            metallic: 1.0,
            perceptual_roughness: 0.32,
            ..Default::default()
        },
    );
    let stall = StallMats {
        wood: mats.wood,
        canvas: mats.canvas,
        iron: mats.iron,
        crystal: mats.crystal,
        pottery: mats.pottery,
        burlap: mats.burlap,
        lamp_glass: mats.lamp_glass,
        produce: mats.pottery,
        cloth,
        wicker: mats.wood,
        steel: mats.metal,
        brass,
    };
    for e in build_stall(world, &stall, at, Goods::Trader, 42) {
        if world.get::<MeshRenderer>(e).is_some() {
            world.entity_mut(e).insert(StaticBackdrop);
        }
    }
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
