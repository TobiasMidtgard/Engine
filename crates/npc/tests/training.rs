//! Sparring posts: the same telegraphed swing, five skill levels. Faster, better defenders
//! block more, masters parry, rookies get hit.

use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_npc::*;
use engine_world::WorldPlugin;

fn app() -> App {
    let mut app = App::new();
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: 1.0 / 60.0,
        });
    app.add_plugins((WorldPlugin, NpcPlugin));
    // Running each system in turn on the test's own thread is far quicker than handing them
    // about a thread pool, and changes nothing (the order is fixed: `tests/schedule_order.rs`).
    for (_, schedule) in app.world.resource_mut::<Schedules>().iter_mut() {
        schedule.set_executor(engine_core::ecs::schedule::SingleThreadedExecutor::new());
    }
    app
}

#[test]
fn skill_decides_who_blocks_the_post() {
    let mut app = app();
    let mut posts = Vec::new();
    for (i, skills) in ["rookie", "soldier", "veteran", "elite", "master"]
        .into_iter()
        .enumerate()
    {
        let at = DVec3::new(i as f64 * 4.0, 0.0, 0.0);
        let npc = app
            .world
            .spawn((
                Transform::from_translation(at),
                Npc {
                    active: false,
                    ..Default::default()
                },
                Faction(23),
                NpcSkills::preset(skills).unwrap(),
                NpcWeapon::sword_and_shield(),
            ))
            .id();
        let post = app
            .world
            .spawn((
                Transform::from_translation(at - DVec3::Z * 1.7),
                Faction(23),
                SparringPost::new(npc),
            ))
            .id();
        posts.push((skills, post, npc));
    }
    app.update_n(60 * 60);
    let mut shares = Vec::new();
    for &(name, post, npc) in &posts {
        let p = *app.world.get::<SparringPost>(post).unwrap();
        let landed = p.blocked + p.parried + p.dodged + p.hits;
        eprintln!(
            "{name}: swings {} blocked {} parried {} dodged {} hits {} ({:.0}% defended), health {}",
            p.swings,
            p.blocked,
            p.parried,
            p.dodged,
            p.hits,
            p.defended_share() * 100.0,
            app.world.get::<NpcVitals>(npc).unwrap().health
        );
        assert!(landed >= 15, "{name}: only {landed} swings landed");
        shares.push(p.defended_share());
    }
    let master = app.world.get::<SparringPost>(posts[4].1).unwrap();
    assert!(shares[0] < 0.2, "rookie {:?}", shares);
    assert!(shares[4] > 0.85, "master {:?}", shares);
    assert!(shares[1] < shares[3] && shares[2] < shares[4], "{shares:?}");
    assert!(master.parried >= 3, "the master should parry: {master:?}");
}
