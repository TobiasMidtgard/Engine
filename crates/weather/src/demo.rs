//! The weather station: a showcase of everything the weather module does, around one point.
//!
//! * A dark asphalt yard where rain pools into puddles that mirror the sky and the lamps,
//!   next to a sand pit that soaks everything up and a stone patio in between.
//! * A shelter roof: the ground under it stays dry, rain stops at its edge and splashes on
//!   top of it.
//! * A mast with a windsock that swings downwind and lifts with the wind speed, and a cup
//!   anemometer that spins faster in gusts.
//! * A thermometer, a rain gauge that fills during showers and a snow stake that measures
//!   the snow lying on the ground.
//! * Street lamps that switch on at dusk, so night rain glitters in their light.
//! * A chrome ball that reflects the clouds.
//!
//! Drive it from the console: `weather.set {"preset": "storm"}`, `weather.time {"hour": 21}`.

use engine_core::math::{DVec3, Quat, Vec3, smoothstep};
use engine_core::prelude::*;
use engine_render::{MeshRenderer, PointLight, Shape, StandardMaterial};
use engine_world::{SurfaceLayers, WeatherState, WindField};

use crate::sky::SkyLights;
use crate::surface::{GroundPatch, WeatherShelter};

/// Points downwind and lifts with the wind speed.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct WindSock;

/// Spins with the wind speed.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Anemometer {
    pub angle: f32,
}

/// Red column of a thermometer: `full_m` tall at `max_c`, empty at `min_c`.
#[derive(Component, Debug, Clone, Copy)]
pub struct ThermometerColumn {
    pub base: DVec3,
    pub full_m: f32,
    pub min_c: f32,
    pub max_c: f32,
}

/// Water column of a rain gauge, filled by rain falling on it, emptied slowly.
#[derive(Component, Debug, Clone, Copy)]
pub struct RainGauge {
    pub base: DVec3,
    pub full_m: f32,
    /// Rain that fills the gauge, mm.
    pub capacity_mm: f32,
    pub collected_mm: f32,
}

/// White drum around a snow stake showing the snow depth on the ground there.
#[derive(Component, Debug, Clone, Copy)]
pub struct SnowStake {
    pub base: DVec3,
}

/// A lamp that switches on when the sun is low.
#[derive(Component, Debug, Clone, Copy)]
pub struct StreetLamp {
    pub lumens: f32,
}

fn mesh(shape: Shape, material: StandardMaterial) -> MeshRenderer {
    MeshRenderer::new(shape, material)
}

fn cuboid(x: f32, y: f32, z: f32) -> Shape {
    Shape::Cuboid { size: [x, y, z] }
}

fn cylinder(radius: f32, height: f32) -> Shape {
    Shape::Cylinder {
        radius,
        height,
        segments: 20,
    }
}

/// Build the weather station around `origin` (on the ground).
pub fn spawn_demo(commands: &mut Commands, origin: DVec3) {
    let at = |x: f64, y: f64, z: f64| Transform::from_translation(origin + DVec3::new(x, y, z));
    let asphalt = StandardMaterial::hex(0x2b2d30).with_roughness(0.85);
    let sand = StandardMaterial::hex(0xd6c094).with_roughness(0.95);
    let stone = StandardMaterial::hex(0x8d8a83).with_roughness(0.8);
    let metal = StandardMaterial::hex(0xb8bcc2)
        .with_metallic(1.0)
        .with_roughness(0.35);
    let dark_metal = StandardMaterial::hex(0x3a3d42)
        .with_metallic(0.8)
        .with_roughness(0.5);
    let white = StandardMaterial::hex(0xf2f2ee).with_roughness(0.6);
    let wood = StandardMaterial::hex(0x7a5534).with_roughness(0.8);

    // Ground: asphalt yard (pools), sand pit (soaks), stone patio (in between).
    for (x, w, d, material, patch) in [
        (
            0.0,
            14.0,
            10.0,
            asphalt,
            GroundPatch {
                half_extents: [7.0, 5.0],
                absorbency: 0.03,
                puddles: 0.5,
            },
        ),
        (
            9.6,
            5.0,
            6.0,
            sand,
            GroundPatch {
                half_extents: [2.5, 3.0],
                absorbency: 1.0,
                puddles: 0.05,
            },
        ),
        (
            -9.6,
            5.0,
            6.0,
            stone,
            GroundPatch {
                half_extents: [2.5, 3.0],
                absorbency: 0.15,
                puddles: 0.3,
            },
        ),
    ] {
        // Flush with the ground: only the top shows.
        commands.spawn((
            at(x, -0.013, 0.0),
            mesh(cuboid(w, 0.03, d), material),
            patch,
        ));
    }

    // Shelter: four posts and a roof; the ground under it stays dry.
    let roof = DVec3::new(-3.5, 2.8, -2.0);
    for (dx, dz) in [(-2.0, -1.5), (2.0, -1.5), (-2.0, 1.5), (2.0, 1.5)] {
        commands.spawn((
            at(roof.x + dx, 1.4, roof.z + dz),
            mesh(cylinder(0.07, 2.8), dark_metal),
        ));
    }
    commands.spawn((
        at(roof.x, roof.y, roof.z),
        mesh(
            cuboid(4.8, 0.14, 3.8),
            StandardMaterial::hex(0x5b6e7a).with_roughness(0.55),
        ),
        WeatherShelter {
            half_extents: [2.4, 1.9],
        },
    ));
    commands.spawn((
        at(roof.x, 0.45, roof.z - 1.0),
        mesh(cuboid(2.6, 0.08, 0.5), wood),
    ));
    for dx in [-1.1, 1.1] {
        commands.spawn((
            at(roof.x + dx, 0.22, roof.z - 1.0),
            mesh(cuboid(0.08, 0.44, 0.45), dark_metal),
        ));
    }

    // Wind mast: windsock and cup anemometer.
    let mast = DVec3::new(4.5, 0.0, -3.5);
    commands.spawn((at(mast.x, 3.0, mast.z), mesh(cylinder(0.06, 6.0), metal)));
    let sock = commands.spawn((at(mast.x, 5.2, mast.z), WindSock)).id();
    commands.spawn((
        Transform::from_xyz(0.0, 0.0, 0.35)
            .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
        mesh(cylinder(0.02, 0.7), metal),
        ChildOf(sock),
    ));
    // The sock: a cone lying along +Z (downwind), wide end at the mast.
    commands.spawn((
        Transform::from_xyz(0.0, 0.0, 1.35)
            .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        mesh(
            Shape::Cone {
                radius: 0.22,
                height: 1.6,
                segments: 20,
            },
            StandardMaterial::hex(0xff5a1f).with_roughness(0.7),
        ),
        ChildOf(sock),
    ));
    let cups = commands
        .spawn((at(mast.x, 6.12, mast.z), Anemometer::default()))
        .id();
    for k in 0..3 {
        let a = k as f32 * std::f32::consts::TAU / 3.0;
        let dir = Vec3::new(a.cos(), 0.0, a.sin());
        commands.spawn((
            Transform::from_translation((dir * 0.22).as_dvec3())
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, dir)),
            mesh(cylinder(0.012, 0.44), metal),
            ChildOf(cups),
        ));
        commands.spawn((
            Transform::from_translation((dir * 0.44).as_dvec3()),
            mesh(
                Shape::Sphere {
                    radius: 0.07,
                    segments: 16,
                },
                white,
            ),
            ChildOf(cups),
        ));
    }

    // Thermometer.
    let thermo = DVec3::new(1.8, 0.0, 4.2);
    commands.spawn((
        at(thermo.x, 1.25, thermo.z - 0.05),
        mesh(cuboid(0.36, 2.3, 0.06), white),
    ));
    commands.spawn((
        at(thermo.x, 1.3, thermo.z),
        mesh(
            cylinder(0.045, 1.9),
            StandardMaterial::color(0.9, 0.95, 1.0)
                .with_roughness(0.05)
                .with_alpha(0.25),
        ),
    ));
    let red = StandardMaterial::hex(0xd0202a)
        .with_roughness(0.3)
        .with_emissive([1.0, 0.1, 0.1], 40.0);
    commands.spawn((
        at(thermo.x, 0.32, thermo.z),
        mesh(
            Shape::Sphere {
                radius: 0.08,
                segments: 16,
            },
            red,
        ),
    ));
    commands.spawn((
        at(thermo.x, 0.4, thermo.z),
        mesh(cylinder(0.028, 1.0), red),
        ThermometerColumn {
            base: origin + DVec3::new(thermo.x, 0.38, thermo.z),
            full_m: 1.8,
            min_c: -30.0,
            max_c: 45.0,
        },
    ));

    // Rain gauge.
    let gauge = DVec3::new(-1.8, 0.0, 4.2);
    commands.spawn((
        at(gauge.x, 0.25, gauge.z),
        mesh(cylinder(0.16, 0.5), dark_metal),
    ));
    commands.spawn((
        at(gauge.x, 0.95, gauge.z),
        mesh(
            cylinder(0.11, 0.9),
            StandardMaterial::color(0.9, 0.95, 1.0)
                .with_roughness(0.05)
                .with_alpha(0.22),
        ),
    ));
    commands.spawn((
        at(gauge.x, 0.5, gauge.z),
        mesh(
            cylinder(0.1, 1.0),
            StandardMaterial::hex(0x2a7fd4)
                .with_roughness(0.1)
                .with_alpha(0.8),
        ),
        RainGauge {
            base: origin + DVec3::new(gauge.x, 0.5, gauge.z),
            full_m: 0.88,
            capacity_mm: 30.0,
            collected_mm: 0.0,
        },
    ));

    // Snow stake: striped pole and a drum showing the snow depth there.
    let stake = DVec3::new(6.0, 0.0, 3.8);
    for k in 0..6 {
        let color = if k % 2 == 0 { 0x101010 } else { 0xf4f4f4 };
        commands.spawn((
            at(stake.x, 0.125 + k as f64 * 0.25, stake.z),
            mesh(cylinder(0.04, 0.25), StandardMaterial::hex(color)),
        ));
    }
    commands.spawn((
        at(stake.x, 0.0, stake.z),
        mesh(
            cylinder(0.5, 1.0),
            StandardMaterial::hex(0xf6f8fb).with_roughness(0.8),
        ),
        SnowStake {
            base: origin + DVec3::new(stake.x, 0.0, stake.z),
        },
    ));

    // Chrome ball on a plinth: reflects the clouds and the sky.
    commands.spawn((at(0.0, 0.4, 1.2), mesh(cylinder(0.25, 0.8), stone_plinth())));
    commands.spawn((
        at(0.0, 1.25, 1.2),
        mesh(
            Shape::Sphere {
                radius: 0.45,
                segments: 48,
            },
            StandardMaterial::hex(0xe8e8e8)
                .with_metallic(1.0)
                .with_roughness(0.05),
        ),
    ));

    // Street lamps.
    for (x, z) in [(-6.5, 4.5), (6.5, -4.5), (-1.0, -4.6)] {
        commands.spawn((at(x, 2.0, z), mesh(cylinder(0.06, 4.0), dark_metal)));
        commands.spawn((
            at(x, 4.05, z),
            mesh(
                Shape::Sphere {
                    radius: 0.18,
                    segments: 16,
                },
                StandardMaterial::hex(0xfff1d6).with_emissive([1.0, 0.85, 0.6], 800.0),
            ),
        ));
        commands.spawn((
            at(x, 3.85, z),
            PointLight {
                color: [1.0, 0.82, 0.58],
                intensity_lm: 0.0,
                range: 18.0,
                radius: 0.15,
            },
            StreetLamp { lumens: 2500.0 },
        ));
    }
}

fn stone_plinth() -> StandardMaterial {
    StandardMaterial::hex(0x9c978e).with_roughness(0.7)
}

/// Update: animate the station's instruments from the weather.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub(crate) fn instruments_system(
    time: Res<Time>,
    wind: Option<Res<WindField>>,
    state: Res<WeatherState>,
    surface: Res<SurfaceLayers>,
    lights: Res<SkyLights>,
    mut socks: Query<
        (&mut Transform, &GlobalTransform),
        (
            With<WindSock>,
            Without<Anemometer>,
            Without<ThermometerColumn>,
        ),
    >,
    mut cups: Query<
        (&mut Transform, &GlobalTransform, &mut Anemometer),
        (Without<WindSock>, Without<ThermometerColumn>),
    >,
    mut columns: Query<
        (&mut Transform, &ThermometerColumn),
        (Without<WindSock>, Without<Anemometer>, Without<RainGauge>),
    >,
    mut gauges: Query<
        (&mut Transform, &mut RainGauge),
        (
            Without<WindSock>,
            Without<Anemometer>,
            Without<ThermometerColumn>,
            Without<SnowStake>,
        ),
    >,
    mut stakes: Query<
        (&mut Transform, &SnowStake),
        (
            Without<WindSock>,
            Without<Anemometer>,
            Without<ThermometerColumn>,
            Without<RainGauge>,
        ),
    >,
    mut lamps: Query<(&mut PointLight, &StreetLamp)>,
) {
    let dt = time.delta_secs();
    let wind_at = |p: DVec3| wind.as_ref().map(|w| w.sample(p)).unwrap_or(state.wind);
    for (mut t, gt) in &mut socks {
        let w = wind_at(gt.translation());
        let flat = Vec3::new(w.x, 0.0, w.z);
        let speed = flat.length();
        // Hangs at ~70° in calm air, flies level at ~12 m/s.
        let lift = smoothstep(0.5, 12.0, speed);
        let droop = (1.0 - lift) * 1.2;
        let heading = if speed > 0.05 {
            flat / speed
        } else {
            t.rotation * Vec3::Z
        };
        let yaw = heading.x.atan2(heading.z);
        let target = Quat::from_rotation_y(yaw) * Quat::from_rotation_x(droop);
        t.rotation = t.rotation.slerp(target, (dt * 3.0).min(1.0));
    }
    for (mut t, gt, mut a) in &mut cups {
        let speed = Vec3::new(1.0, 0.0, 1.0) * wind_at(gt.translation());
        // Cup anemometers turn at roughly a third of the wind speed at the cups (r = 0.44 m).
        a.angle = (a.angle + speed.length() * 0.35 / 0.44 * dt).rem_euclid(std::f32::consts::TAU);
        t.rotation = Quat::from_rotation_y(a.angle);
    }
    for (mut t, c) in &mut columns {
        let f = ((state.temperature_c - c.min_c) / (c.max_c - c.min_c)).clamp(0.02, 1.0);
        let h = c.full_m * f;
        t.scale = Vec3::new(1.0, h, 1.0);
        t.translation = c.base + DVec3::new(0.0, h as f64 * 0.5, 0.0);
    }
    for (mut t, mut g) in &mut gauges {
        let raining = matches!(
            state.precipitation_kind,
            engine_world::PrecipitationKind::Rain
                | engine_world::PrecipitationKind::Sleet
                | engine_world::PrecipitationKind::Hail
        );
        let rate = if raining {
            state.precipitation_mm_h
        } else {
            0.0
        };
        // Fills with the rain, evaporates ~1 mm/h, a little faster in the sun.
        let evap = 1.0 + 2.0 * state.daylight();
        g.collected_mm = (g.collected_mm + (rate - evap) * dt / 3600.0).clamp(0.0, g.capacity_mm);
        let h = (g.full_m * g.collected_mm / g.capacity_mm).max(0.002);
        t.scale = Vec3::new(1.0, h, 1.0);
        t.translation = g.base + DVec3::new(0.0, h as f64 * 0.5, 0.0);
    }
    for (mut t, s) in &mut stakes {
        let depth = surface.sample(s.base).snow_m.min(1.5);
        let h = depth.max(0.0);
        t.scale = Vec3::new(1.0, h.max(0.0001), 1.0);
        t.translation = s.base + DVec3::new(0.0, h as f64 * 0.5, 0.0);
    }
    // Lamps come on in the dusk and in dark storms.
    let dark = smoothstep(4.0, -2.0, lights.sun_elevation_deg)
        .max(smoothstep(0.85, 0.98, state.cloud_cover) * smoothstep(0.5, 1.0, state.storm));
    for (mut l, lamp) in &mut lamps {
        l.intensity_lm = lamp.lumens * dark;
    }
}
