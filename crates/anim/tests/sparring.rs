//! The paddock's sparring partner holds a boxer's guard, snaps his head back from the boxer's
//! jab and folds a little over a blow to the stomach, then comes back to the guard, without
//! being shoved off his feet; the boxer's combo lands.

use engine_anim::combat::Attacker;
use engine_anim::demo::{DemoEntities, spawn_demo_world};
use engine_anim::{AnimationPlugin, Animator};
use engine_core::prelude::*;
use engine_core::time::TimeSource;
use engine_world::WorldPlugin;

const STEP: f64 = 1.0 / 60.0;

fn app() -> (App, DemoEntities) {
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    app.world
        .resource_mut::<Time>()
        .set_source(TimeSource::Manual { step_secs: STEP });
    app.add_plugins((WorldPlugin, AnimationPlugin));
    let demo = spawn_demo_world(&mut app.world, DVec3::ZERO);
    engine_anim::character::finish_loading(&mut app.world);
    (app, demo)
}

/// A bone's position in the person's own frame (they face −Z), m.
fn local(world: &World, e: Entity, bone: &str) -> Vec3 {
    let a = world.get::<Animator>(e).unwrap();
    a.model[a.skeleton.bone(bone)].t
}

/// How far the head is tipped back (+) or forward (−), degrees.
fn head_pitch(world: &World, e: Entity) -> f32 {
    let a = world.get::<Animator>(e).unwrap();
    let up = a.model[a.skeleton.bone("head")].r * Vec3::Y;
    up.z.atan2(up.y).to_degrees()
}

#[test]
fn sparring_partner_guards_and_flinches() {
    let (mut app, d) = app();
    let e = d.sparring;
    let (mut guard_pitch, mut snap) = (0.0f32, f32::MIN);
    // How far the neck is ahead of the chest (m, − forward): in the guard, then most.
    let (mut lean, mut fold) = (0.0f32, 0.0f32);
    let (mut lowest_hips, mut farthest_hips) = (f32::MAX, 0.0f32);
    // The boxer's combo lands on the head from about 0.6 s; the stomach is struck at 3.8 s
    // and the back of a shoulder at 5 s.
    let (mut hits, mut last, mut first_hit) = (0, 0, None::<f64>);
    for frame in 1..=(6.0 / STEP) as usize {
        app.update();
        let t = frame as f64 * STEP;
        let w = &app.world;
        let landed = w.get::<Attacker>(d.boxer).unwrap().hits;
        if landed != last {
            if landed > last {
                hits += landed - last;
                first_hit.get_or_insert(t);
            }
            last = landed;
        }
        let hips = local(w, e, "hips");
        if t > 0.5 {
            lowest_hips = lowest_hips.min(hips.y);
            farthest_hips = farthest_hips.max(hips.with_y(0.0).length());
        }
        if (0.3..0.4).contains(&t) {
            guard_pitch = head_pitch(w, e);
            lean = (local(w, e, "neck") - local(w, e, "chest")).z;
            // Fists up by the chin, the lead (left) one out in front.
            let (lead, rear) = (local(w, e, "hand_l"), local(w, e, "hand_r"));
            let chin = local(w, e, "head").y - 0.12;
            assert!(
                lead.y > chin - 0.1 && rear.y > chin - 0.1,
                "fists at {:.2} and {:.2} m, chin at {chin:.2}",
                lead.y,
                rear.y
            );
            assert!(
                lead.z < rear.z - 0.08,
                "lead fist at {lead}, rear at {rear}"
            );
        }
        if first_hit.is_some_and(|h| t - h < 0.3) {
            snap = snap.max(head_pitch(w, e) - guard_pitch);
        }
        if (3.8..4.0).contains(&t) {
            fold = fold.min((local(w, e, "neck") - local(w, e, "chest")).z);
        }
    }
    assert!(hits >= 4, "only {hits} of the combo's punches landed");
    assert!(
        first_hit.is_some_and(|h| h < 0.8),
        "the jab landed at {first_hit:?} s"
    );
    assert!(snap > 15.0, "the head snapped back only {snap:.1}°");
    assert!(
        fold < lean - 0.02,
        "the chest only folded from {lean:.3} to {fold:.3} m forward"
    );
    // The hips give a little, but he stays on his feet.
    assert!(lowest_hips > 0.75, "hips sank to {lowest_hips:.2} m");
    assert!(farthest_hips < 0.12, "hips shoved {farthest_hips:.2} m");
}
