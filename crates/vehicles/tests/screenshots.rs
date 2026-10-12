//! Renders the vehicles offscreen: a garage shot of the cars, a hangar shot of the aircraft,
//! the race track, airfield and spaceport demos, each after the scene has run a while on the
//! stand-in solver. Needs a GPU or a software Vulkan driver (lavapipe); passes with a note
//! when neither is available.
//!
//! PNGs go to `CARGO_TARGET_TMPDIR`, or to `$VEHICLE_SHOTS` when set. The helpers live in
//! `support/shots.rs`.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::*;
use support::shots::*;
use support::*;

#[test]
fn cars_in_the_garage() {
    shoot(
        "cars",
        2.0,
        |app| {
            floor(app, 200.0);
            parked(app, "sports_car", DVec3::new(-2.0, 0.0, 0.0), 35.0);
            parked(app, "rally_car", DVec3::new(3.2, 0.0, -3.5), -25.0);
        },
        Transform::from_xyz(-7.5, 2.2, 7.5).looking_at(DVec3::new(0.5, 0.6, -1.0), Vec3::Y),
    );
}

#[test]
fn aircraft_on_the_apron() {
    shoot(
        "aircraft",
        2.0,
        |app| {
            floor(app, 300.0);
            parked(app, "sport_plane", DVec3::new(-9.0, 0.0, 4.0), 30.0);
            parked(app, "spaceplane", DVec3::new(10.0, 0.0, -8.0), -30.0);
        },
        Transform::from_xyz(-24.0, 7.0, 26.0).looking_at(DVec3::new(2.0, 2.0, -3.0), Vec3::Y),
    );
}

#[test]
fn race_track_demo() {
    shoot(
        "track",
        8.0,
        |app| {
            plane(app, 900.0, -0.01, 0x4f6b32, 0.95);
            spawn_with(app, |c| {
                engine_vehicles::demo::spawn_track(c, DVec3::ZERO);
                c.spawn_empty().id()
            });
        },
        Transform::from_xyz(-30.0, 80.0, 170.0).looking_at(DVec3::new(0.0, 0.0, 5.0), Vec3::Y),
    );
}

#[test]
fn airfield_demo() {
    shoot(
        "airfield",
        1.5,
        |app| {
            plane(app, 1600.0, -0.01, 0x4f6b32, 0.95);
            spawn_with(app, |c| {
                engine_vehicles::demo::spawn_airfield(
                    c,
                    DVec3::new(0.0, 0.0, 600.0),
                    DVec3::new(0.0, 0.0, -600.0),
                );
                c.spawn_empty().id()
            });
        },
        Transform::from_xyz(22.0, 7.0, 612.0).looking_at(DVec3::new(0.0, 1.5, 578.0), Vec3::Y),
    );
}

#[test]
fn spaceport_demo() {
    shoot(
        "spaceport",
        9.0,
        |app| {
            plane(app, 600.0, -0.01, 0x6f6a5e, 0.9);
            // The pad itself is drawn by the game; a plain one here.
            spawn_with(app, |c| {
                c.spawn((
                    Transform::from_xyz(0.0, 0.2, 0.0),
                    MeshRenderer::new(
                        Shape::Cylinder {
                            radius: 30.0,
                            height: 0.4,
                            segments: 64,
                        },
                        StandardMaterial::hex(0x9a9a96).with_roughness(0.8),
                    ),
                ))
                .id()
            });
            spawn_with(app, |c| {
                engine_vehicles::demo::spawn_spaceport(c, DVec3::new(0.0, 0.4, 0.0), 30.0);
                c.spawn_empty().id()
            });
        },
        Transform::from_xyz(55.0, 18.0, 62.0).looking_at(DVec3::new(-10.0, 10.0, 0.0), Vec3::Y),
    );
}
