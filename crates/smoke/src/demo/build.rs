//! Puts the range together: registers its textures, materials and meshes (once), then spawns
//! the scenery, the gun, the fans with their rotors, the cart with its wheelsets, the grenades
//! and the steam vent around the demo's origin.

use std::f32::consts::FRAC_PI_2;

use engine_core::math::{DVec3, Quat, Vec2, Vec3};
use engine_core::prelude::*;

use super::art::{self, Parts};
use super::kit::at;
use super::{
    BLAST, DemoCart, DemoFan, DemoGrenade, DemoRotor, DemoWheel, FOOTPRINT, MUZZLE, SmokeDemo,
    SmokeDemoState, TARGET_Y, TARGET_Z, TARGETS_X, extras, fan, firing, grass, ground, gun,
    targets, track, vent, warm,
};
use crate::sim::{SmokeFan, SmokeObstacle, SmokeSource, SmokeVolume};

/// Where the cart's track runs (z) and how far (x, ±).
pub const TRACK_Z: f64 = 5.5;
pub const TRACK_HALF: f32 = 17.6;
/// The cart's travel (x) and the height of its entity.
const CART_FROM: f64 = -15.0;
const CART_TO: f64 = 15.0;
const CART_Y: f64 = 1.0;
/// Fans: z positions along the west side, hub height and x.
const FAN_Z: [f64; 3] = [-6.0, 2.0, 10.0];
const FAN_X: f64 = -17.4;
const FAN_Y: f64 = 2.4;
/// The steam vent's outlet.
const VENT: [f64; 3] = [14.0, 0.6, -10.0];

/// The grenades: position (range space) and the source each round starts with.
fn grenades() -> [([f64; 3], SmokeSource); 4] {
    [
        ([-2.5, 0.15, 1.0], {
            // Burns long enough that the fans still have a screen to push.
            let mut g = SmokeSource::grenade([0.92, 0.92, 0.9], 32.0);
            g.rate_kg_s = 0.12;
            g.radius = 0.6;
            g.temperature_c = 32.0;
            g.gas_m3_s = 22.0;
            g
        }),
        (
            [-9.0, 0.15, 12.0],
            warm(SmokeSource::grenade([0.85, 0.12, 0.08], 28.0)),
        ),
        (
            [0.0, 0.15, 13.5],
            warm(SmokeSource::grenade([0.45, 0.16, 0.7], 28.0)),
        ),
        (
            [9.0, 0.15, 12.0],
            warm(SmokeSource::grenade([0.12, 0.7, 0.18], 28.0)),
        ),
    ]
}

/// Everything that never moves, in range space.
fn scenery() -> Parts {
    let mut p = Parts::default();
    let [hx, hz] = FOOTPRINT;
    let mut spots: Vec<(Vec2, f32, f32)> = grenades()
        .iter()
        .map(|(g, _)| (Vec2::new(g[0] as f32, g[2] as f32), 1.6, 0.1))
        .collect();
    for &x in &TARGETS_X {
        // Lead dust and fragments sprayed in front of every target.
        spots.push((Vec2::new(x as f32, TARGET_Z as f32 - 0.8), 2.2, 0.18));
    }
    spots.push((
        Vec2::new(MUZZLE[0] as f32, MUZZLE[2] as f32 - 1.3),
        1.2,
        0.2,
    ));
    let blast = Vec2::new(BLAST[0] as f32, BLAST[2] as f32);
    ground::pad(&mut p, [hx, hz], blast, &spots);
    let vent_at = Vec2::new(VENT[0] as f32, VENT[2] as f32);
    let tz = TRACK_Z as f32;
    ground::markings(&mut p, vent_at, [tz - 2.15, tz + 2.15]);
    ground::decal(&mut p, art::SCORCH, blast, 3.4, 0.7, 0.004);
    for (i, (g, _)) in grenades().iter().enumerate() {
        ground::decal(
            &mut p,
            art::SCORCH,
            Vec2::new(g[0] as f32, g[2] as f32) + Vec2::new(0.1, -0.05),
            0.95 + i as f32 * 0.07,
            i as f32 * 1.9,
            0.0035 + i as f32 * 0.0002,
        );
    }
    // Oil: drips under the fans' motors, by the vent and the gun, and old stains about.
    let mut oil_spots: Vec<(Vec2, f32)> = FAN_Z
        .iter()
        .map(|z| (Vec2::new(FAN_X as f32 + 0.35, *z as f32 - 0.2), 0.55))
        .collect();
    oil_spots.extend([
        (vent_at + Vec2::new(0.9, 0.7), 0.7),
        (
            Vec2::new(MUZZLE[0] as f32 + 1.9, MUZZLE[2] as f32 - 2.6),
            0.6,
        ),
        (Vec2::new(-7.5, -11.0), 0.9),
        (Vec2::new(4.2, -4.5), 0.75),
        (Vec2::new(10.5, -15.0), 1.0),
        (Vec2::new(-12.0, 8.5), 0.6),
        (Vec2::new(13.5, 2.0), 0.8),
        (Vec2::new(-3.0, 9.0), 0.5),
    ]);
    for (i, (c, r)) in oil_spots.into_iter().enumerate() {
        ground::decal(
            &mut p,
            art::OIL,
            c,
            r,
            i as f32 * 2.3,
            0.0025 + i as f32 * 0.0001,
        );
    }
    let xs: Vec<f32> = TARGETS_X.iter().map(|x| *x as f32).collect();
    ground::berm(&mut p, hx, &xs);
    targets::targets(&mut p, &xs, TARGET_Y as f32, TARGET_Z as f32);
    // Firing line: barriers either side of the gun pit, sandbags around the gun.
    let fz = MUZZLE[2] as f32 - 0.5;
    firing::barrier_row(&mut p, -hx + 3.4, -2.75, fz, 0xBA_01);
    firing::barrier_row(&mut p, 2.75, hx - 3.4, fz, 0xBA_02);
    let pit = Vec2::new(MUZZLE[0] as f32, MUZZLE[2] as f32 - 1.42);
    firing::sandbag_arc(&mut p, pit, 1.45, [-1.95, 1.95], 4, 2, 0x5A_0D);
    // Sandbags weighing down the fan stands.
    for (i, z) in FAN_Z.iter().enumerate() {
        for (k, (dx, dz)) in [(0.5f32, 0.5f32), (0.5, -0.5)].iter().enumerate() {
            firing::loose_bag(
                &mut p,
                Vec3::new(FAN_X as f32 + dx, ground::PAD_TOP + 0.03, *z as f32 + dz),
                (dz / dx).atan() + FRAC_PI_2,
                (i * 7 + k) as u64 + 0xFA,
            );
        }
    }
    track::track(&mut p, tz, TRACK_HALF);
    // Clutter: fan power cables, range flags, ammunition behind the gun.
    let fz: Vec<f32> = FAN_Z.iter().map(|z| *z as f32).collect();
    extras::fan_cables(
        &mut p,
        FAN_X as f32,
        FAN_Y as f32,
        &fz,
        Vec2::new(-18.45, -13.5),
    );
    extras::flag(&mut p, Vec2::new(-17.0, -20.8), 71);
    extras::flag(&mut p, Vec2::new(17.0, -20.8), 73);
    extras::ammo_dump(&mut p, Vec2::new(-2.9, -21.2), 0xA770);
    // The gravel shoulder and grass round the pad, on the berm and in the joints.
    let mut tufts = ground::shoulder(&mut p, [hx, hz]);
    tufts.extend(ground::berm_tufts(hx, 0x6_BE));
    tufts.extend(ground::weeds([hx, hz], 0x6_EED));
    grass::scatter(&mut p, &tufts, 0x6_7F);
    vent::vent(
        &mut p,
        Vec3::new(VENT[0] as f32, VENT[1] as f32, VENT[2] as f32),
        hx,
    );
    p
}

/// Spawn one entity per registered part of `group` at `transform`.
fn spawn_group(
    world: &mut World,
    name: &str,
    group: &str,
    transform: Transform,
    parts: impl FnOnce() -> Parts,
) {
    for (mesh, mat) in art::register(world, group, parts) {
        world.spawn((
            Name::new(name.to_string()),
            transform,
            art::renderer(&mesh, mat),
        ));
    }
}

/// Build the whole range around `origin` (see `super::spawn_demo`).
pub fn build(world: &mut World, origin: DVec3) {
    let started = std::time::Instant::now();
    art::ensure_materials(world);
    let demo = world
        .spawn((
            Name::new("Smoke demo"),
            Transform::from_translation(origin),
            SmokeDemo::default(),
            SmokeDemoState::default(),
        ))
        .id();
    let at_w = |x: f64, y: f64, z: f64| Transform::from_translation(origin + DVec3::new(x, y, z));
    let [hx, hz] = FOOTPRINT;

    // The air over the range: 0.5 m voxels, 14 m high.
    world.spawn((
        Name::new("Smoke demo: air"),
        at_w(0.0, 0.0, 0.0),
        SmokeVolume {
            size: [hx * 2.0, 14.0, hz * 2.0],
            cell_m: 0.5,
        },
    ));

    spawn_group(
        world,
        "Smoke demo: range",
        "range",
        at_w(0.0, 0.0, 0.0),
        scenery,
    );
    let [mx, my, mz] = MUZZLE;
    spawn_group(
        world,
        "Smoke demo: turret",
        "gun",
        at_w(mx, my, mz),
        gun::gun,
    );

    // Grenades: the white screen and the signal colors by the targets.
    for (i, (p, source)) in grenades().into_iter().enumerate() {
        let mut idle = source;
        idle.active = false;
        let g = world
            .spawn((
                Name::new(format!("Smoke demo: grenade {i}")),
                at_w(p[0], p[1], p[2]),
                idle,
                DemoGrenade {
                    demo,
                    fresh: source,
                },
            ))
            .id();
        let color = Vec3::from(source.color);
        let floor = ground::PAD_TOP - p[1] as f32;
        let parts = art::register(world, &format!("grenade{i}"), || {
            vent::grenade(color, floor, 0x6E0 + i as u64)
        });
        art::spawn_parts(world, g, &parts);
    }

    // Steam vent: always on.
    world.spawn((
        Name::new("Smoke demo: steam vent"),
        at_w(VENT[0], VENT[1], VENT[2]),
        SmokeSource {
            rate_kg_s: 0.012,
            radius: 0.25,
            temperature_c: 100.0,
            velocity: [0.0, 4.0, 0.0],
            color: [0.97, 0.97, 0.97],
            ..Default::default()
        },
    ));

    // Fans along the west side, blowing east across the range.
    let fan_ground = ground::PAD_TOP - FAN_Y as f32;
    let body = art::register(world, "fan_body", || fan::body(fan_ground));
    let rotor = art::register(world, "fan_rotor", fan::rotor);
    for z in FAN_Z {
        let f = world
            .spawn((
                Name::new("Smoke demo: fan"),
                at_w(FAN_X, FAN_Y, z).with_rotation(Quat::from_rotation_y(-FRAC_PI_2)),
                SmokeFan {
                    speed: 9.0,
                    radius: 2.4,
                    reach: 26.0,
                    on: false,
                },
                DemoFan { demo },
            ))
            .id();
        art::spawn_parts(world, f, &body);
        let r = world
            .spawn((
                Name::new("Smoke demo: fan rotor"),
                Transform::default(),
                DemoRotor {
                    fan: f,
                    ..Default::default()
                },
                ChildOf(f),
            ))
            .id();
        art::spawn_parts(world, r, &rotor);
    }

    // The cart on its track.
    let drop = track::RAIL_TOP - CART_Y as f32;
    let cart_body = art::register(world, "cart_body", || {
        let mut p = Parts::default();
        p.absorb(track::cart_body(), at(Vec3::new(0.0, drop, 0.0)));
        p
    });
    let wheels = art::register(world, "cart_wheelset", track::wheelset);
    let cart = world
        .spawn((
            Name::new("Smoke demo: cart"),
            at_w(CART_FROM, CART_Y, TRACK_Z),
            SmokeObstacle { radius: 1.8 },
            DemoCart {
                from: origin + DVec3::new(CART_FROM, CART_Y, TRACK_Z),
                to: origin + DVec3::new(CART_TO, CART_Y, TRACK_Z),
                speed: 3.5,
                phase: 0.0,
            },
        ))
        .id();
    art::spawn_parts(world, cart, &cart_body);
    for ax in [-track::AXLE_X, track::AXLE_X] {
        let w = world
            .spawn((
                Name::new("Smoke demo: wheelset"),
                Transform::from_xyz(ax as f64, (drop + track::WHEEL_R) as f64, 0.0),
                DemoWheel {
                    cart,
                    radius: track::WHEEL_R,
                    ..Default::default()
                },
                ChildOf(cart),
            ))
            .id();
        art::spawn_parts(world, w, &wheels);
    }
    tracing::debug!(
        "smoke range built in {:.2} s",
        started.elapsed().as_secs_f32()
    );
}
