//! Mars' dust haze against an independent reference: dark and bright ground patches seen
//! through `Atmosphere::mars()` from orbit (straight down, and obliquely onto a 21 km summit)
//! and from the ground (a canyon wall 25 km away from a floor 6 km below datum, a ridge 30 km
//! away from datum). Each case is rendered twice: from far away, and from 150 m (where the
//! renderer adds no aerial perspective, so the patches show their own light). From the two,
//! the test measures what the air in between does: the share of the patches' contrast that
//! survives (their transmittance) and the light the air adds (the haze), in linear color with
//! the tone curve off. A CPU reference computes both for the same air: single scattering by
//! quadrature, plus multiple scattering (dust, gas and ground bounces) by a deterministic
//! Monte Carlo in a plane-parallel atmosphere. Needs a GPU or lavapipe; passes with a note when
//! neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, Renderer, capture_now};
use image::RgbaImage;

const W: u32 = 160;
const H: u32 = 120;
/// Sunlight above the air (Mars' mean, lux).
const SUN_LUX: f32 = 50_000.0;
const EV100: f32 = 12.5;
const DARK: f32 = 0.06;
const BRIGHT: f32 = 0.4;
/// How far from the patches the reference shot is taken: close enough that the renderer adds
/// no aerial perspective.
const NEAR: f64 = 150.0;

/// One view of the patches.
struct Case {
    name: &'static str,
    /// The camera.
    eye: DVec3,
    /// Where the dark and the bright patch meet.
    target: DVec3,
    /// The patches' normal, and the direction from the dark patch to the bright one.
    normal: Vec3,
    across: Vec3,
    /// Size of each (square) patch, meters.
    patch: f32,
    /// Toward the sun.
    to_sun: DVec3,
    /// A floor under the camera (world y), for views from the ground.
    floor: Option<f64>,
    /// The ground under the multiply scattered light, height above datum (reference only).
    ground_h: f64,
    fov: f32,
}

fn mars() -> Atmosphere {
    Atmosphere::mars()
}

fn sun(elevation_deg: f64, azimuth_deg: f64) -> DVec3 {
    // Azimuth from +z (behind a camera looking along -z), toward +x.
    let (e, a) = (elevation_deg.to_radians(), azimuth_deg.to_radians());
    DVec3::new(a.sin() * e.cos(), e.sin(), a.cos() * e.cos())
}

/// The four cases.
fn cases() -> [Case; 4] {
    let air = mars();
    // (b): a camera 400 km up seeing a summit 21 km up 60° from its zenith.
    let summit = DVec3::new(0.0, 21_000.0, 0.0);
    let zenith = 60f64.to_radians();
    let u = DVec3::new(0.0, zenith.cos(), zenith.sin());
    let oc = summit - air.planet_center;
    let r = air.planet_radius + 400_000.0;
    // |oc + u t| = r
    let b = oc.dot(u);
    let t = -b + (b * b - (oc.length_squared() - r * r)).sqrt();
    [
        Case {
            name: "a_orbit_down",
            eye: DVec3::new(0.0, 400_000.0, 0.0),
            target: DVec3::ZERO,
            normal: Vec3::Y,
            across: Vec3::X,
            patch: 30_000.0,
            to_sun: sun(45.0, 0.0),
            floor: None,
            ground_h: 0.0,
            fov: 8.0,
        },
        Case {
            name: "b_orbit_summit",
            eye: summit + u * t,
            target: summit,
            normal: Vec3::Y,
            across: Vec3::X,
            patch: 30_000.0,
            to_sun: sun(45.0, 60.0),
            floor: None,
            ground_h: 0.0,
            fov: 8.0,
        },
        Case {
            name: "c_canyon_wall",
            eye: DVec3::new(0.0, -5_998.0, 0.0),
            target: DVec3::new(0.0, -5_500.0, -25_000.0),
            normal: Vec3::Z,
            across: Vec3::X,
            patch: 8_000.0,
            to_sun: sun(45.0, 30.0),
            floor: Some(-6_000.0),
            ground_h: -6_000.0,
            fov: 8.0,
        },
        Case {
            name: "d_datum_ridge",
            eye: DVec3::new(0.0, 2.0, 0.0),
            target: DVec3::new(0.0, 500.0, -30_000.0),
            normal: Vec3::Z,
            across: Vec3::X,
            patch: 8_000.0,
            to_sun: sun(45.0, 30.0),
            floor: Some(0.0),
            ground_h: 0.0,
            fov: 8.0,
        },
    ]
}

fn render(case: &Case, eye: DVec3, name: &str) -> Option<(App, RgbaImage)> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.saturation = 1.0;
        s.contrast = 1.0;
        s.white_balance = 0.0;
        s.anti_aliasing = AntiAliasing::None;
        s.volumetric_fog = false;
        s.sky_cloud_layer = false;
        s.ambient_occlusion = false;
        s.screen_space_gi = false;
        s.screen_space_reflections = false;
    }
    app.world.resource_mut::<FogSettings>().density = 0.0;
    app.world.insert_resource(AmbientLight {
        color: [1.0, 1.0, 1.0],
        brightness: 0.0,
    });
    *app.world.resource_mut::<Atmosphere>() = mars();
    let w = &mut app.world;
    let view = (case.target - eye).normalize();
    let up = if view.y.abs() > 0.9 {
        DVec3::NEG_Z
    } else {
        DVec3::Y
    };
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: EV100 },
            ..Camera::perspective(case.fov)
        },
        Transform::from_translation(eye).looking_at(case.target, up.as_vec3()),
    ));
    w.spawn((
        DirectionalLight {
            color: [1.0, 1.0, 1.0],
            illuminance: SUN_LUX,
            ..Default::default()
        },
        Transform::default().looking_at(-case.to_sun, Vec3::Y),
    ));
    let rotation = Quat::from_rotation_arc(Vec3::Y, case.normal);
    for (albedo, side) in [(DARK, -1.0), (BRIGHT, 1.0)] {
        let center = case.target + (case.across * case.patch * 0.5 * side).as_dvec3();
        w.spawn((
            Transform::from_translation(center).with_rotation(rotation),
            MeshRenderer::new(
                Shape::Plane {
                    size: [case.patch, case.patch],
                    subdivisions: 1,
                },
                StandardMaterial {
                    reflectance: 0.0,
                    ..StandardMaterial::color(albedo, albedo, albedo).with_roughness(1.0)
                },
            ),
        ));
    }
    if let Some(y) = case.floor {
        w.spawn((
            Transform::from_xyz(0.0, y, -20_000.0),
            MeshRenderer::new(
                Shape::Plane {
                    size: [80_000.0, 80_000.0],
                    subdivisions: 1,
                },
                StandardMaterial {
                    reflectance: 0.0,
                    ..StandardMaterial::color(0.2, 0.2, 0.2).with_roughness(1.0)
                },
            ),
        ));
    }
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    Some((app, img))
}

/// Mean linear color of a 5x5 block around where world point `p` lands.
fn color_at(app: &App, img: &RgbaImage, p: DVec3) -> Vec3 {
    let view = app
        .world
        .resource::<Renderer>()
        .last_view()
        .unwrap()
        .clone();
    let clip = view.unjittered_view_proj * (p - view.origin).as_vec3().extend(1.0);
    let ndc = clip.truncate() / clip.w;
    let x = ((ndc.x * 0.5 + 0.5) * W as f32) as i32;
    let y = ((0.5 - ndc.y * 0.5) * H as f32) as i32;
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let mut sum = Vec3::ZERO;
    for dy in -2..=2 {
        for dx in -2..=2 {
            let px = img.get_pixel(
                (x + dx).clamp(0, W as i32 - 1) as u32,
                (y + dy).clamp(0, H as i32 - 1) as u32,
            );
            sum += Vec3::new(lin(px[0]), lin(px[1]), lin(px[2])) / 25.0;
        }
    }
    sum
}

/// Linear pixel value per cd/m².
fn exposure() -> f32 {
    1.0 / (1.2 * 2f32.powf(EV100))
}

/// The dark and the bright patch's linear colors, a little to either side of where they meet.
fn patches(case: &Case, app: &App, img: &RgbaImage, eye: DVec3) -> (Vec3, Vec3) {
    let dist = (case.target - eye).length();
    let off = (dist * (case.fov as f64 * 0.5).to_radians().tan() * 0.5) as f32;
    let at = |side: f32| case.target + (case.across * off * side).as_dvec3();
    (color_at(app, img, at(-1.0)), color_at(app, img, at(1.0)))
}

/// What the renderer's air does between the camera and the patches: transmittance and haze
/// radiance (per lux of sunlight, RGB), the haze relative to the patches' own light, and
/// the patches' contrast (Michelson) far away over that close up.
struct Measured {
    transmittance: Vec3,
    haze: Vec3,
    retention: Vec3,
    near_dark: Vec3,
    near_bright: Vec3,
}

fn michelson(dark: Vec3, bright: Vec3) -> Vec3 {
    (bright - dark) / (bright + dark).max(Vec3::splat(1e-6))
}

fn measure(case: &Case) -> Option<Measured> {
    let near_eye = case.target + (case.eye - case.target).normalize() * NEAR;
    let (app, img) = render(case, case.eye, &format!("mars_haze_{}", case.name))?;
    let (far_dark, far_bright) = patches(case, &app, &img, case.eye);
    let (app, img) = render(case, near_eye, &format!("mars_haze_{}_near", case.name))?;
    let (near_dark, near_bright) = patches(case, &app, &img, near_eye);
    let transmittance = (far_bright - far_dark) / (near_bright - near_dark).max(Vec3::splat(1e-6));
    let haze = (far_dark - near_dark * transmittance) / (exposure() * SUN_LUX);
    Some(Measured {
        transmittance,
        haze,
        retention: michelson(far_dark, far_bright) / michelson(near_dark, near_bright),
        near_dark,
        near_bright,
    })
}

// ------------------------------------------------------------------ the CPU reference

/// A small PCG random number generator (deterministic across platforms).
struct Pcg(u64);

impl Pcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let x = (((self.0 >> 18) ^ self.0) >> 27) as u32;
        let rot = (self.0 >> 59) as u32;
        (x.rotate_right(rot) as f64 + 0.5) / 4_294_967_296.0
    }
}

/// One color channel of the air, in a plane-parallel atmosphere (heights above datum; below
/// datum the air is as dense as at datum, as the renderer has it).
struct Air {
    rayleigh: f64,
    mie_scattering: f64,
    mie_extinction: f64,
    rayleigh_h: f64,
    mie_h: f64,
    g: f64,
    top: f64,
    ground_albedo: f64,
}

impl Air {
    fn channel(a: &Atmosphere, c: usize) -> Self {
        let k = a.density_scale as f64;
        Self {
            rayleigh: a.rayleigh_scattering[c] as f64 * k,
            mie_scattering: a.mie_scattering[c] as f64 * k,
            mie_extinction: (a.mie_scattering[c] + a.mie_absorption[c]) as f64 * k,
            rayleigh_h: a.rayleigh_scale_height as f64,
            mie_h: a.mie_scale_height as f64,
            g: (a.mie_g + a.mie_g_offset[c]) as f64,
            top: a.thickness as f64,
            ground_albedo: a.ground_color[c] as f64,
        }
    }

    fn density(&self, z: f64, h: f64) -> f64 {
        (-z.clamp(0.0, self.top) / h).exp() * if z > self.top { 0.0 } else { 1.0 }
    }

    /// The integral of the density of scale height `h` from height z0 to z1 (z0 <= z1).
    fn column(&self, z0: f64, z1: f64, h: f64) -> f64 {
        let (z0, z1) = (z0.min(self.top), z1.min(self.top));
        if z1 <= z0 {
            return 0.0;
        }
        let below = (z1.min(0.0) - z0.min(0.0)).max(0.0);
        let (a, b) = (z0.max(0.0), z1.max(0.0));
        below + h * ((-a / h).exp() - (-b / h).exp())
    }

    fn extinction_at(&self, z: f64) -> f64 {
        self.rayleigh * self.density(z, self.rayleigh_h)
            + self.mie_extinction * self.density(z, self.mie_h)
    }

    /// Optical depth from height z for `len` meters along a direction with vertical
    /// component `dz`.
    fn depth(&self, z: f64, dz: f64, len: f64) -> f64 {
        if dz.abs() < 1e-7 {
            return self.extinction_at(z) * len;
        }
        let z1 = z + dz * len;
        let (lo, hi) = (z.min(z1), z.max(z1));
        (self.rayleigh * self.column(lo, hi, self.rayleigh_h)
            + self.mie_extinction * self.column(lo, hi, self.mie_h))
            / dz.abs()
    }

    /// Sunlight reaching height z (sun's vertical component `sz` > 0).
    fn sun(&self, z: f64, sz: f64) -> f64 {
        (-self.depth(z, sz, (self.top - z).max(0.0) / sz)).exp()
    }

    /// Phase functions: Rayleigh and the renderer's Mie (Cornette-Shanks).
    fn phase_rayleigh(nu: f64) -> f64 {
        3.0 / (16.0 * std::f64::consts::PI) * (1.0 + nu * nu)
    }

    fn phase_mie(&self, nu: f64) -> f64 {
        let g2 = self.g * self.g;
        3.0 / (8.0 * std::f64::consts::PI) * (1.0 - g2) * (1.0 + nu * nu)
            / ((2.0 + g2) * (1.0 + g2 - 2.0 * self.g * nu).max(1e-9).powf(1.5))
    }

    /// Where along a direction the optical depth from z reaches `tau` (at most `max_len`).
    fn distance_to_depth(&self, z: f64, dz: f64, tau: f64, max_len: f64) -> f64 {
        let (mut lo, mut hi) = (0.0, max_len);
        for _ in 0..48 {
            let mid = 0.5 * (lo + hi);
            if self.depth(z, dz, mid) < tau {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }
}

/// The reference for one channel: transmittance along the view, single-scattered and
/// multiply scattered haze radiance (per unit sunlight), and the standard error of the latter.
struct Reference {
    transmittance: f64,
    single: f64,
    multiple: f64,
    multiple_error: f64,
}

/// `eye_h`: the camera's height; `view`, `sun`: unit directions with z up; `dist`: to the
/// target; `ground_h`: the ground the light bounces off.
fn reference(
    air: &Air,
    eye_h: f64,
    view: DVec3,
    dist: f64,
    sun: DVec3,
    ground_h: f64,
) -> Reference {
    use std::f64::consts::PI;
    // From where the view enters the air.
    let (z0, len) = if eye_h > air.top {
        let skip = (eye_h - air.top) / -view.z;
        (air.top, dist - skip)
    } else {
        (eye_h, dist)
    };
    let nu = view.dot(sun);
    let tau_view = air.depth(z0, view.z, len);
    // Single scattering by quadrature.
    let n = 4000;
    let mut single = 0.0;
    for i in 0..n {
        let t = (i as f64 + 0.5) / n as f64 * len;
        let z = z0 + view.z * t;
        let source = (air.rayleigh * air.density(z, air.rayleigh_h) * Air::phase_rayleigh(nu)
            + air.mie_scattering * air.density(z, air.mie_h) * air.phase_mie(nu))
            * air.sun(z, sun.z);
        single += source * (-air.depth(z0, view.z, t)).exp() * len / n as f64;
    }
    // Multiple scattering: backward Monte Carlo from the camera with next-event estimation
    // toward the sun (and off the ground), the first collision forced onto the view segment.
    let paths = 6000;
    let mut rng = Pcg(0x5eed_1234_abcd_0001);
    let (mut sum, mut sum2) = (0.0, 0.0);
    let collide = 1.0 - (-tau_view).exp();
    for _ in 0..paths {
        let mut estimate = 0.0;
        let tau = -(1.0 - rng.next() * collide).ln();
        let mut z = z0 + view.z * air.distance_to_depth(z0, view.z, tau, len);
        let mut dir = view;
        let mut weight = collide;
        for order in 1..64 {
            // A collision at height z, arriving along `dir`.
            let sr = air.rayleigh * air.density(z, air.rayleigh_h);
            let sm = air.mie_scattering * air.density(z, air.mie_h);
            weight *= (sr + sm) / air.extinction_at(z).max(1e-30);
            let p_rayleigh = sr / (sr + sm).max(1e-30);
            if order > 1 {
                let c = dir.dot(sun);
                let phase =
                    p_rayleigh * Air::phase_rayleigh(c) + (1.0 - p_rayleigh) * air.phase_mie(c);
                estimate += weight * phase * air.sun(z, sun.z);
            }
            // New direction: Rayleigh from a uniform proposal, Mie from Henyey-Greenstein,
            // weighted by the true phase over the proposal.
            let (u, v) = (rng.next(), rng.next());
            let pick_rayleigh = rng.next() < p_rayleigh;
            let (cos_t, ratio) = if pick_rayleigh {
                let c = 1.0 - 2.0 * u;
                (c, Air::phase_rayleigh(c) * 4.0 * PI)
            } else {
                let g = air.g;
                let s = (1.0 - g * g) / (1.0 - g + 2.0 * g * u);
                let c = ((1.0 + g * g - s * s) / (2.0 * g)).clamp(-1.0, 1.0);
                let hg = (1.0 - g * g) / (4.0 * PI * (1.0 + g * g - 2.0 * g * c).powf(1.5));
                (c, air.phase_mie(c) / hg)
            };
            weight *= ratio;
            dir = rotate(dir, cos_t, 2.0 * PI * v);
            // Fly to the next collision, bouncing off the ground on the way.
            loop {
                let to_bound = if dir.z > 1e-9 {
                    (air.top - z) / dir.z
                } else if dir.z < -1e-9 {
                    (ground_h - z) / dir.z
                } else {
                    1e9
                };
                let tau_bound = air.depth(z, dir.z, to_bound);
                let tau = -(1.0 - rng.next()).ln();
                if tau < tau_bound {
                    z += dir.z * air.distance_to_depth(z, dir.z, tau, to_bound);
                    break;
                }
                if dir.z >= 0.0 {
                    weight = 0.0;
                    break;
                }
                // The ground: Lambertian, lit by the sun, then a new cosine-weighted direction.
                z = ground_h;
                estimate += weight * air.ground_albedo / PI * sun.z * air.sun(z, sun.z);
                weight *= air.ground_albedo;
                let (a, b) = (rng.next(), 2.0 * PI * rng.next());
                let r = a.sqrt();
                dir = DVec3::new(r * b.cos(), r * b.sin(), (1.0 - a).sqrt());
                if weight < 1e-4 {
                    break;
                }
            }
            if weight < 1e-4 {
                break;
            }
            if weight < 0.02 {
                if rng.next() < 0.5 {
                    break;
                }
                weight *= 2.0;
            }
        }
        sum += estimate;
        sum2 += estimate * estimate;
    }
    let mean = sum / paths as f64;
    let var = (sum2 / paths as f64 - mean * mean).max(0.0);
    Reference {
        transmittance: (-tau_view).exp(),
        single,
        multiple: mean,
        multiple_error: (var / paths as f64).sqrt(),
    }
}

/// `dir` turned by an angle of cosine `cos_t`, at azimuth `phi` around itself.
fn rotate(dir: DVec3, cos_t: f64, phi: f64) -> DVec3 {
    let a = if dir.z.abs() < 0.9 {
        DVec3::Z
    } else {
        DVec3::X
    };
    let b1 = dir.cross(a).normalize();
    let b2 = dir.cross(b1);
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    (dir * cos_t + b1 * (sin_t * phi.cos()) + b2 * (sin_t * phi.sin())).normalize()
}

/// The reference for a case, RGB: transmittance, haze (single + multiple) and its error.
fn reference_rgb(case: &Case) -> (Vec3, Vec3, Vec3, Vec3) {
    let a = mars();
    let height = |p: DVec3| (p - a.planet_center).length() - a.planet_radius;
    let view = (case.target - case.eye).normalize();
    let dist = (case.target - case.eye).length();
    // The plane-parallel atmosphere is the one under the camera, or from orbit the one under
    // the patches (where the view crosses the air).
    let from_orbit = height(case.eye) > a.thickness as f64;
    let anchor = if from_orbit { case.target } else { case.eye };
    let up = (anchor - a.planet_center).normalize();
    // A frame with z along the local vertical.
    let east = up
        .cross(if up.x.abs() < 0.9 { DVec3::X } else { DVec3::Z })
        .normalize();
    let north = up.cross(east);
    let local = |d: DVec3| DVec3::new(d.dot(east), d.dot(north), d.dot(up));
    let eye_h = if from_orbit {
        height(case.target) - local(view).z * dist
    } else {
        height(case.eye)
    };
    let (mut t, mut single, mut multiple, mut err) =
        ([0.0f32; 3], [0.0f32; 3], [0.0f32; 3], [0.0f32; 3]);
    for c in 0..3 {
        let air = Air::channel(&a, c);
        let r = reference(
            &air,
            eye_h,
            local(view),
            dist,
            local(case.to_sun.normalize()),
            case.ground_h,
        );
        t[c] = r.transmittance as f32;
        single[c] = r.single as f32;
        multiple[c] = r.multiple as f32;
        err[c] = r.multiple_error as f32;
    }
    (
        Vec3::from(t),
        Vec3::from(single),
        Vec3::from(multiple),
        Vec3::from(err),
    )
}

#[test]
fn mars_haze_matches_the_reference() {
    let mut failures: Vec<String> = Vec::new();
    for case in cases() {
        let Some(m) = measure(&case) else {
            return;
        };
        let (t, single, multiple, err) = reference_rgb(&case);
        let haze = single + multiple;
        // The contrast the reference keeps, for the patches' own light as rendered close up.
        let near_d = m.near_dark / (exposure() * SUN_LUX);
        let near_b = m.near_bright / (exposure() * SUN_LUX);
        let retention = michelson(near_d * t + haze, near_b * t + haze) / michelson(near_d, near_b);
        eprintln!(
            "{}: transmittance {:.3} (reference {:.3}); haze {:.5} (reference {:.5} = single {:.5} + multiple {:.5} ± {:.5}); \
             contrast kept {:.3} (reference {:.3}); patches close up {:.4} / {:.4}",
            case.name,
            m.transmittance,
            t,
            m.haze,
            haze,
            single,
            multiple,
            err,
            m.retention,
            retention,
            near_d,
            near_b,
        );
        // The air must let through what the reference lets through: a far darker image from
        // orbit means the ground lost light, not that the air took it.
        let dt = (m.transmittance - t).abs().max_element();
        if dt > 0.04 {
            failures.push(format!(
                "{}: transmittance {:.3} vs the reference's {t:.3}",
                case.name, m.transmittance
            ));
        }
        // The haze: at least the single scattering and most of the multiple, at most a little
        // more than both (the multiple scattering is a Monte Carlo estimate, within 3 standard
        // errors).
        let low = single + (multiple - err * 3.0) * 0.6;
        let high = (haze + err * 3.0) * 1.3;
        if m.haze.cmplt(low).any() || m.haze.cmpgt(high).any() {
            failures.push(format!(
                "{}: haze {:.5} outside {low:.5}..{high:.5} (reference {haze:.5})",
                case.name, m.haze
            ));
        }
        // And the contrast that survives: no less than the reference keeps.
        if (m.retention - retention).min_element() < -0.05 {
            failures.push(format!(
                "{}: contrast kept {:.3} vs the reference's {retention:.3}",
                case.name, m.retention
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
