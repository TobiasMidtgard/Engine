//! Freeflow chaining on a ring of dummies: a fighter tapping the light attack at a steady
//! rhythm, the stick on the next dummy once the one before goes down, clears four of them
//! at 1 to 5 m in every direction. Each press goes at the dummy the stick points at; a far
//! one is closed on with a lunge or a leap (never faster than the leap's cap); a dummy that
//! goes down hands the fight over at once, so the next blow starts within a beat of the kill;
//! no blow whiffs and the combo never breaks. Runs without the fetched models.

use std::collections::HashMap;

use engine_anim::combat::Attacker;
use engine_anim::demo::spawn_humanoid;
use engine_anim::{
    AnimationPlugin, Animator, Freeflow, HitReaction, Knockdown, build::humanoid::Outfit,
};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;

const DT: f32 = 1.0 / 60.0;

/// Blows that put a dummy down.
const BLOWS: u32 = 3;

#[test]
fn a_ring_of_dummies_goes_down_in_one_unbroken_chain() {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual {
            step_secs: DT as f64,
        });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let fighter = spawn_humanoid(&mut app.world, Outfit::Athlete, Transform::default());
    app.world
        .entity_mut(fighter)
        .insert((Attacker::new("boxing", None), Freeflow::default()));
    // Ahead at 1.1 m, to the left at 3 m, to the right at 5 m, behind at 2 m; each faces the
    // fighter.
    let spots = [
        Vec3::new(0.0, 0.0, -1.1),
        Vec3::new(-3.0, 0.0, 0.0),
        Vec3::new(5.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 2.0),
    ];
    let dummies: Vec<Entity> = spots
        .iter()
        .map(|&at| {
            let face = Quat::from_rotation_y((at.x).atan2(at.z));
            let d = spawn_humanoid(
                &mut app.world,
                Outfit::Shirt,
                Transform::from_translation(at.as_dvec3()).with_rotation(face),
            );
            app.world.entity_mut(d).insert(HitReaction::default());
            d
        })
        .collect();
    for _ in 0..30 {
        app.update();
    }

    let mut taken: HashMap<Entity, u32> = HashMap::new();
    let mut moves: Vec<(String, Option<Entity>, f32)> = Vec::new();
    let mut downs: Vec<(Entity, f32)> = Vec::new();
    let (mut last_hits, mut last_missed, mut missed) = (0, 0, 0);
    let mut peak_speed: f32 = 0.0;
    let mut was: (Option<String>, f32) = (None, 0.0);
    let mut best_combo = 0;
    let mut t = 0.0;
    let mut next_press = 0.1;
    while t < 10.0 && downs.len() < dummies.len() {
        let me = app
            .world
            .get::<GlobalTransform>(fighter)
            .unwrap()
            .translation();
        // The stick points at the next dummy still up (in the ring's order).
        let up = dummies
            .iter()
            .copied()
            .find(|d| !downs.iter().any(|(e, _)| e == d));
        if let Some(d) = up {
            let at = app.world.get::<GlobalTransform>(d).unwrap().translation();
            app.world.get_mut::<Freeflow>(fighter).unwrap().stick = (at - me).as_vec3();
        }
        // A steady tap of the light attack.
        if t >= next_press {
            app.world
                .get_mut::<Animator>(fighter)
                .unwrap()
                .trigger("attack_light");
            next_press += 0.22;
        }
        app.update();
        t += DT;
        let a = app.world.get::<Attacker>(fighter).unwrap().clone();
        peak_speed = peak_speed.max(a.warp_delta.length() / DT);
        best_combo = best_combo.max(app.world.get::<Freeflow>(fighter).unwrap().combo);
        let started = a.current.is_some() && (a.current != was.0 || a.time < was.1);
        was = (a.current.clone(), a.time);
        if started {
            moves.push((a.current.clone().unwrap(), a.struck, t));
            last_hits = 0;
            last_missed = 0;
        }
        if a.hits > last_hits {
            let victim = a.struck.unwrap();
            let n = taken.entry(victim).or_default();
            *n += a.hits - last_hits;
            last_hits = a.hits;
            // Down on the third blow (no physics here, so knocked down by hand, as the
            // health rules of a game would).
            if *n >= BLOWS && !downs.iter().any(|(e, _)| *e == victim) {
                app.world.entity_mut(victim).insert(Knockdown::default());
                downs.push((victim, t));
            }
        }
        if a.missed > last_missed {
            missed += a.missed - last_missed;
            last_missed = a.missed;
        }
    }
    // (The last blow counts toward the combo a frame later.)
    for _ in 0..3 {
        app.update();
        best_combo = best_combo.max(app.world.get::<Freeflow>(fighter).unwrap().combo);
    }
    let names: Vec<&str> = moves.iter().map(|m| m.0.as_str()).collect();
    assert_eq!(
        downs.len(),
        4,
        "only {} of 4 down in {t:.1} s; moves {names:?}",
        downs.len()
    );
    // Each in the order the stick pointed.
    let order: Vec<Entity> = downs.iter().map(|d| d.0).collect();
    assert_eq!(order, dummies, "moves {names:?}");
    // The far ones were closed on: a lunge to the left one (3 m off), a leap to the right one
    // (about 7 m on from the left one) and another back to the one behind (about 5 m on from
    // the right one); the near one needed none.
    let approach_at = |d: Entity| -> Vec<&str> {
        moves
            .iter()
            .filter(|m| m.1 == Some(d) && (m.0 == "lunge" || m.0 == "leap"))
            .map(|m| m.0.as_str())
            .collect()
    };
    assert_eq!(approach_at(dummies[0]), Vec::<&str>::new(), "{names:?}");
    assert_eq!(approach_at(dummies[1]), ["lunge"], "{names:?}");
    assert_eq!(approach_at(dummies[2]), ["leap"], "{names:?}");
    assert_eq!(approach_at(dummies[3]), ["leap"], "{names:?}");
    // A kill hands over at once: the next move, at the next dummy, starts within a tap and a
    // hitstop of it.
    for w in downs.windows(2) {
        let (killed, at) = w[0];
        let next = moves
            .iter()
            .find(|m| m.2 > at && m.1 != Some(killed))
            .unwrap_or_else(|| panic!("nothing after the kill at {at:.2} s: {names:?}"));
        assert!(
            next.2 - at <= 0.3,
            "{:.2} s from the kill at {at:.2} s to the {} at the next",
            next.2 - at,
            next.0
        );
    }
    let gaps: Vec<String> = downs
        .windows(2)
        .filter_map(|w| {
            let next = moves.iter().find(|m| m.2 > w[0].1 && m.1 != Some(w[0].0))?;
            Some(format!("{:.2}", next.2 - w[0].1))
        })
        .collect();
    println!(
        "cleared in {t:.2} s; kill to next move {gaps:?} s; peak warp {peak_speed:.1} m/s; \
         best combo {best_combo}; moves {names:?}"
    );
    assert_eq!(missed, 0, "{missed} blows whiffed: {names:?}");
    assert!(peak_speed <= 18.0 + 1e-3, "warped at {peak_speed} m/s");
    // Twelve blows, never more than 1.5 s apart.
    assert!(best_combo >= 12, "best combo {best_combo}: {names:?}");
}
