//! Every vehicle with an engine reports its exhausts for the effects modules: where each
//! nozzle is, which way the gas goes, and how hard it fires this frame. Stand-in plumes step
//! aside while an effects module draws them.

mod support;

use engine_core::math::{DVec3, Vec3};
use engine_core::prelude::*;
use engine_render::MeshRenderer;
use engine_vehicles::presets;
use engine_vehicles::visuals::PlumeVisual;
use engine_vehicles::{Spacecraft, ThrusterRole, Vehicle, VehicleKind};
use engine_world::{ExhaustEffects, ExhaustKind, Exhausts};
use support::*;

fn exhausts(app: &App, e: Entity) -> Exhausts {
    app.world.get::<Exhausts>(e).expect("exhausts").clone()
}

fn on_ground(app: &mut App, kind: &str) -> Entity {
    calm(app);
    ground(app, DVec3::ZERO, 500.0, "asphalt");
    let t = Transform::from_translation(DVec3::new(0.0, presets::ride_height(kind), 0.0));
    let e = spawn_with(app, |c| presets::spawn(c, kind, t).unwrap());
    run_for(app, 0.5);
    e
}

#[test]
fn the_spaceplane_reports_a_nozzle_per_thruster_at_its_exit() {
    let mut app = vehicle_app();
    let ship = on_ground(&mut app, "spaceplane");
    let thrusters = app.world.get::<Spacecraft>(ship).unwrap().thrusters.clone();
    let ex = exhausts(&app, ship);
    assert_eq!(
        ex.0.len(),
        thrusters.len(),
        "one nozzle per thruster, in order"
    );
    for (n, t) in ex.0.iter().zip(&thrusters) {
        // The gas leaves against the push, from the model's nozzle exit near the thruster.
        assert!(
            n.direction.dot(-t.direction.normalize()) > 0.7,
            "{}: gas {:?} against push {:?}",
            t.name,
            n.direction,
            t.direction
        );
        assert!(
            n.position.distance(t.position) < 2.0,
            "{} at {:?}",
            t.name,
            n.position
        );
        let want = match t.role {
            ThrusterRole::Rcs => ExhaustKind::Thruster,
            _ => ExhaustKind::Rocket,
        };
        assert_eq!(n.kind, want, "{}", t.name);
        if t.role == ThrusterRole::Main {
            // The bell rims, behind the tail.
            assert!(n.position.z > 11.0 && n.radius > 0.6, "{n:?}");
        }
        // Parked on its gear, the flight assist doesn't trim with the RCS.
        assert!(
            n.level < 0.005,
            "{} idles on the ground at {}",
            t.name,
            n.level
        );
    }

    // Main engines lit: their nozzles fire, the others stay idle.
    controls(&mut app, ship, |c| c.throttle = 1.0);
    run_for(&mut app, 2.0);
    let ex = exhausts(&app, ship);
    let ship_now = app.world.get::<Spacecraft>(ship).unwrap();
    for (n, t) in ex.0.iter().zip(&ship_now.thrusters) {
        assert_eq!(n.level, t.level, "{} follows its throttle", t.name);
        if t.role == ThrusterRole::Main {
            assert!(n.level > 0.9, "{} at {}", t.name, n.level);
        }
    }
}

#[test]
fn a_ship_built_from_data_gets_a_nozzle_at_each_thruster() {
    let mut app = vehicle_app();
    calm(&mut app);
    let thrusters = presets::spaceplane_thrusters();
    let ship = spawn_with(&mut app, |c| {
        c.spawn((
            Transform::from_translation(DVec3::new(0.0, 100.0, 0.0)),
            Vehicle::new(
                "probe",
                VehicleKind::Spacecraft,
                1_000.0,
                Vec3::new(2.0, 2.0, 4.0),
            ),
            Spacecraft {
                thrusters: thrusters.clone(),
                ..Default::default()
            },
            engine_world::VehicleControls::default(),
        ))
        .id()
    });
    run_for(&mut app, 0.1);
    let ex = exhausts(&app, ship);
    assert_eq!(ex.0.len(), thrusters.len());
    let main = &ex.0[0];
    assert_eq!(main.position, thrusters[0].position);
    assert_eq!(main.direction, Vec3::Z, "a −Z push blows out the back");
    assert!((0.7..0.85).contains(&main.radius), "{}", main.radius);
}

#[test]
fn car_tailpipes_sit_at_the_back_and_follow_throttle_and_load() {
    for (kind, pipes) in [("sports_car", 2), ("rally_car", 1)] {
        let mut app = vehicle_app();
        let car = on_ground(&mut app, kind);
        let ex = exhausts(&app, car);
        assert_eq!(ex.0.len(), pipes, "{kind}");
        for n in &ex.0 {
            assert_eq!(n.kind, ExhaustKind::Tailpipe);
            assert_eq!(n.direction, Vec3::Z, "{kind} blows out the back");
            assert!(n.position.z > 1.9 && n.position.y < 0.0, "{kind} {n:?}");
            assert!((0.03..0.06).contains(&n.radius), "{kind} {n:?}");
        }
        let idle = ex.0[0].level;
        assert!((0.03..0.2).contains(&idle), "{kind} idles at {idle}");

        controls(&mut app, car, |c| c.throttle = 1.0);
        let mut most = 0.0f32;
        run_until(&mut app, 4.0, |app| {
            most = most.max(exhausts(app, car).0[0].level);
            false
        });
        assert!(most > 0.6, "{kind} flat out only at {most}");
    }
}

#[test]
fn the_sport_plane_reports_its_four_exhaust_stubs() {
    let mut app = vehicle_app();
    let plane = on_ground(&mut app, "sport_plane");
    let ex = exhausts(&app, plane);
    assert_eq!(ex.0.len(), 4);
    for n in &ex.0 {
        assert_eq!(n.kind, ExhaustKind::Tailpipe);
        // Under the cowl, behind the propeller, blowing down and back.
        assert!(n.position.z < -1.5 && n.position.y < -0.3, "{n:?}");
        assert!(n.direction.z > 0.3 && n.direction.y < 0.0, "{n:?}");
    }
    let idle = ex.0[0].level;
    controls(&mut app, plane, |c| {
        c.throttle = 1.0;
        c.brake = 1.0;
    });
    run_for(&mut app, 2.0);
    let full = exhausts(&app, plane).0[0].level;
    assert!(idle < 0.2 && full > 0.8, "idle {idle}, full {full}");
}

#[test]
fn stand_in_plumes_hide_while_an_effects_module_draws_exhausts() {
    let mut app = vehicle_app();
    let ship = on_ground(&mut app, "spaceplane");
    controls(&mut app, ship, |c| c.throttle = 1.0);
    run_for(&mut app, 1.0);
    let lit = |app: &mut App| {
        let mut q = app.world.query::<(&ChildOf, &PlumeVisual, &MeshRenderer)>();
        q.iter(&app.world)
            .filter(|(p, _, r)| p.parent() == ship && r.visible)
            .count()
    };
    assert!(lit(&mut app) >= 3, "the main plumes burn");
    app.insert_resource(ExhaustEffects {
        drawn_by: "vfx".into(),
    });
    run_for(&mut app, 0.1);
    assert_eq!(lit(&mut app), 0, "nothing burns twice");
    app.world.remove_resource::<ExhaustEffects>();
    run_for(&mut app, 0.1);
    assert!(lit(&mut app) >= 3, "back when the effects module goes");
}
