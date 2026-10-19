//! Gallery: what the effects modules get from `Exhausts`, drawn as glowing cones on each
//! nozzle (base on the exit, pointing the way the gas goes, as long as the level is high),
//! with the stand-in plumes put away. Renders only with `VEHICLE_SHOTS=<dir>`.

mod support;

use engine_core::math::{DVec3, Quat, Vec3};
use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_world::{ExhaustEffects, ExhaustKind, Exhausts};
use support::shots::*;
use support::*;

#[derive(Component)]
struct Marker(usize);

fn marker_color(kind: ExhaustKind) -> (f32, f32, f32) {
    match kind {
        ExhaustKind::Rocket | ExhaustKind::Afterburner => (1.0, 0.45, 0.1),
        ExhaustKind::Thruster => (0.2, 0.8, 1.0),
        _ => (1.0, 0.9, 0.2),
    }
}

/// Puts a cone on every nozzle of every vehicle and sizes it with the nozzle's level.
fn markers(
    mut commands: Commands,
    vehicles: Query<(Entity, &Exhausts)>,
    mut cones: Query<(&ChildOf, &Marker, &mut Transform)>,
    mut done: Local<bool>,
) {
    if !*done {
        for (e, ex) in &vehicles {
            for (i, n) in ex.0.iter().enumerate() {
                let (r, g, b) = marker_color(n.kind);
                let glow = StandardMaterial::color(r, g, b).with_emissive([r, g, b], 6.0);
                commands.spawn((
                    Transform::default(),
                    MeshRenderer::new(
                        Shape::Cone {
                            radius: 1.0,
                            height: 1.0,
                            segments: 24,
                        },
                        glow,
                    ),
                    Marker(i),
                    ChildOf(e),
                ));
            }
        }
        *done = !vehicles.is_empty();
    }
    for (parent, m, mut t) in &mut cones {
        let Ok((_, ex)) = vehicles.get(parent.parent()) else {
            continue;
        };
        let Some(n) = ex.0.get(m.0) else { continue };
        let len = n.radius * (1.5 + 10.0 * n.level);
        *t = Transform::from_translation((n.position + n.direction * len * 0.5).as_dvec3())
            .with_rotation(Quat::from_rotation_arc(Vec3::Y, n.direction))
            .with_scale(Vec3::new(n.radius, len, n.radius));
    }
}

fn with_markers(app: &mut App) {
    app.add_systems(Update, markers);
    app.insert_resource(ExhaustEffects {
        drawn_by: "gallery markers".into(),
    });
}

/// The spaceplane with its main engines lit: three bells, hover wells and RCS ports.
#[test]
fn exhausts_spaceplane() {
    gallery_shot(
        "exhausts_spaceplane",
        0.4,
        |app| {
            with_markers(app);
            floor(app, 300.0);
            let ship = parked(app, "spaceplane", DVec3::ZERO, 0.0);
            controls(app, ship, |c| c.throttle = 1.0);
        },
        Transform::from_xyz(-16.0, 7.0, 26.0).looking_at(DVec3::new(0.0, 1.5, 6.0), Vec3::Y),
    );
}

/// Both cars from behind: the sports car flat out against its brakes, the rally car idling.
#[test]
fn exhausts_cars() {
    gallery_shot(
        "exhausts_cars",
        1.5,
        |app| {
            with_markers(app);
            floor(app, 300.0);
            let sports = parked(app, "sports_car", DVec3::new(-1.4, 0.0, 0.0), 0.0);
            controls(app, sports, |c| {
                c.throttle = 1.0;
                c.handbrake = true;
                c.brake = 1.0;
            });
            parked(app, "rally_car", DVec3::new(1.4, 0.0, 0.0), 0.0);
        },
        Transform::from_xyz(1.2, 1.1, 6.5).looking_at(DVec3::new(0.0, 0.3, 1.5), Vec3::Y),
    );
}

/// The sport plane at full power on its brakes: the four stubs under the cowl.
#[test]
fn exhausts_sport_plane() {
    gallery_shot(
        "exhausts_sport_plane",
        1.0,
        |app| {
            with_markers(app);
            floor(app, 300.0);
            let plane = parked(app, "sport_plane", DVec3::ZERO, 0.0);
            controls(app, plane, |c| {
                c.throttle = 1.0;
                c.brake = 1.0;
            });
        },
        Transform::from_xyz(2.2, 0.2, -3.6).looking_at(DVec3::new(0.0, 0.3, -2.0), Vec3::Y),
    );
}
