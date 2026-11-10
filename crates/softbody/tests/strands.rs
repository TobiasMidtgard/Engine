//! Hair and fur reach the shared strand renderer: every hair body Softbody draws gets a valid
//! groom on an entity riding its anchor frame, with posed guides the renderer takes.

use engine_assets::Assets;
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_render::{StrandFiber, StrandGroom, StrandInstance};
use engine_softbody::strand_draw::{DrawnStrands, StrandsOf};
use engine_softbody::{SoftBodySim, demo};
use engine_world::HairGuidesPosed;

fn yard() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((engine_world::WorldPlugin, engine_softbody::SoftBodyPlugin));
    app.world.init_resource::<Assets<StrandGroom>>();
    app.add_systems(Startup, |mut commands: Commands| {
        demo::spawn_demo(&mut commands, DVec3::ZERO);
    });
    app.update();
    engine_core::off_frame::finish(&mut app.world, None);
    // Hair is groomed once settled.
    for _ in 0..240 {
        app.update();
        engine_core::off_frame::finish(&mut app.world, None);
        let mut q = app.world.query::<&SoftBodySim>();
        if q.iter(&app.world).all(|b| b.pending_prewarm == 0) {
            break;
        }
    }
    for _ in 0..3 {
        app.update();
        engine_core::off_frame::finish(&mut app.world, None);
    }
    app
}

#[test]
fn every_drawn_hair_body_has_a_groom_the_renderer_poses() {
    let mut app = yard();
    let mut bodies = app
        .world
        .query::<(Entity, &SoftBodySim, Option<&DrawnStrands>, Option<&Name>)>();
    let hair: Vec<_> = bodies
        .iter(&app.world)
        .filter(|(_, b, _, _)| b.strands.is_some() && !b.drawn_elsewhere)
        .map(|(e, b, d, n)| {
            (
                e,
                d.map(|d| d.entity),
                n.map_or(String::new(), |n| n.as_str().to_string()),
                b.strands.as_ref().unwrap().style.length,
            )
        })
        .collect();
    let names: Vec<_> = hair.iter().map(|h| h.2.clone()).collect();
    // The dog's coat waits for its body mesh, which only a renderer makes: the people's hair.
    assert!(hair.len() >= 2, "the yard has long hair: {names:?}");
    let grooms = app.world.resource::<Assets<StrandGroom>>();
    for (body, drawn, name, length) in &hair {
        let drawn = drawn.unwrap_or_else(|| panic!("{name} has no strands drawn"));
        let e = app.world.entity(drawn);
        assert_eq!(e.get::<StrandsOf>(), Some(&StrandsOf(*body)));
        let inst = e.get::<StrandInstance>().unwrap();
        let groom = grooms.get(inst.groom).unwrap();
        assert!(groom.is_valid(), "{name}");
        assert!(groom.strand_count() > 1000, "{name}");
        let posed = e.get::<HairGuidesPosed>().unwrap();
        assert_eq!(
            posed.points.len(),
            groom.guides.len(),
            "{name}: the renderer only takes posed guides that match"
        );
        // Posed near rest: the instance rides the anchor, so the guides moved little.
        let worst = posed
            .points
            .iter()
            .zip(&groom.guides)
            .map(|(p, r)| p.distance(Vec3::from(*r)))
            .fold(0.0f32, f32::max);
        let per = groom.points_per as usize;
        let roots: f32 = (0..groom.guides.len() / per)
            .map(|g| posed.points[g * per].distance(Vec3::from(groom.guides[g * per])))
            .fold(0.0, f32::max);
        assert!(
            worst < 0.5,
            "{name}: guides {worst} m from rest, roots {roots}: {:?} vs {:?}",
            &posed.points[..3],
            &groom.guides[..3]
        );
        let fiber = if *length < 0.15 {
            StrandFiber::Fur
        } else {
            StrandFiber::Human
        };
        assert_eq!(groom.fiber, fiber, "{name}");
    }
}

#[test]
fn a_hair_body_gone_takes_its_strands_with_it() {
    let mut app = yard();
    let mut q = app.world.query::<(Entity, &DrawnStrands)>();
    let (body, drawn) = q
        .iter(&app.world)
        .map(|(e, d)| (e, d.entity))
        .next()
        .unwrap();
    let handle = app.world.get::<StrandInstance>(drawn).unwrap().groom;
    app.world.despawn(body);
    app.update();
    app.update();
    assert!(app.world.get_entity(drawn).is_err(), "the strands went");
    assert!(
        app.world
            .resource::<Assets<StrandGroom>>()
            .get(handle)
            .is_none(),
        "and their groom"
    );
}
