//! Locomotion warping on the humanoid's own walk: the hips turn toward a diagonal travel while
//! the head keeps facing ahead, a faster body stretches the stride (the hips dropping only as
//! far as the legs need), and nothing changes when standing or while the fallback shows. Runs
//! without the fetched models.

use std::sync::Arc;

use engine_anim::animator::Animator;
use engine_anim::build::humanoid;
use engine_anim::clip::ClipBinding;
use engine_anim::warp::LocomotionWarp;
use engine_core::math::{Quat, Vec3};

const DT: f32 = 1.0 / 60.0;

/// The humanoid mid-stride on its walk (the moment its ankles are farthest apart).
fn walking() -> Animator {
    let s = Arc::new(humanoid::skeleton());
    let clips = humanoid::clips(&s);
    let walk = clips.iter().find(|c| c.name == "walk").unwrap();
    let binding = ClipBinding::new(walk, &s);
    let mut animator = Animator::procedural(s.clone());
    let [l, r] = humanoid::FOOT.map(|f| s.index_of(f).unwrap());
    let mut best = (0.0, 0.0);
    for i in 0..20 {
        let t = walk.duration * i as f32 / 20.0;
        animator.local = s.rest.clone();
        walk.sample(t, &binding, &mut animator.local);
        animator.refresh_model();
        let spread = (animator.model[l].t.z - animator.model[r].t.z).abs();
        if spread > best.0 {
            best = (spread, t);
        }
    }
    animator.local = s.rest.clone();
    walk.sample(best.1, &binding, &mut animator.local);
    animator.refresh_model();
    animator
}

fn legs(a: &Animator) -> Vec<(usize, usize, usize, Vec3)> {
    let s = &a.skeleton;
    (0..2)
        .map(|i| {
            (
                s.index_of(humanoid::UPPER_LEG[i]).unwrap(),
                s.index_of(humanoid::LOWER_LEG[i]).unwrap(),
                s.index_of(humanoid::FOOT[i]).unwrap(),
                Vec3::NEG_Z,
            )
        })
        .collect()
}

fn bone(a: &Animator, name: &str) -> usize {
    a.skeleton.index_of(name).unwrap()
}

/// Which way a model-space rotation faces (yaw, radians; people face −Z).
fn yaw(r: Quat) -> f32 {
    let f = r * Vec3::NEG_Z;
    (-f.x).atan2(-f.z)
}

/// Track the wanted warp for a second, then apply it to a fresh pose.
fn warped(speed: f32, heading: f32, share: f32, stride_speed: f32) -> (Animator, Animator) {
    let before = walking();
    let mut after = walking();
    let mut warp = LocomotionWarp::default();
    for _ in 0..60 {
        warp.track(speed, heading, share, stride_speed, DT);
    }
    let legs = legs(&after);
    for _ in 0..30 {
        // (The hips' drop eases in; each frame starts from the clip's pose again.)
        after = walking();
        warp.apply(&mut after.edit(), heading, &legs, DT);
    }
    (before, after)
}

#[test]
fn a_diagonal_turns_the_hips_and_the_head_keeps_facing_ahead() {
    let heading = 45f32.to_radians();
    let (before, after) = warped(1.4, heading, 1.0, 1.4);
    let turned =
        |name| yaw(after.model[bone(&after, name)].r) - yaw(before.model[bone(&before, name)].r);
    let hips = turned("hips");
    assert!(
        (hips - heading).abs() < 1f32.to_radians(),
        "hips turned {}°",
        hips.to_degrees()
    );
    let (spine, chest, head) = (turned("spine"), turned("chest"), turned("head"));
    assert!(
        head.abs() < 1f32.to_radians(),
        "head turned {}°",
        head.to_degrees()
    );
    assert!(
        chest > 0.0 && chest < 0.4 * heading && spine > chest,
        "spine {}°, chest {}°",
        spine.to_degrees(),
        chest.to_degrees()
    );
    // The feet now step along the diagonal: the line between the ankles turned with the hips.
    let feet = |a: &Animator| {
        let d = a.model[bone(a, "foot_l")].t - a.model[bone(a, "foot_r")].t;
        d.x.atan2(d.z)
    };
    let mut swung = feet(&after) - feet(&before);
    if swung > std::f32::consts::PI {
        swung -= std::f32::consts::TAU;
    }
    if swung < -std::f32::consts::PI {
        swung += std::f32::consts::TAU;
    }
    assert!(
        (swung.abs() - heading).abs() < 3f32.to_radians(),
        "feet line swung {}°",
        swung.to_degrees()
    );
}

#[test]
fn a_faster_body_stretches_the_stride_and_the_hips_drop_a_little() {
    // The body moves 1.3 times as fast as the clip's feet cover the ground.
    let (before, after) = warped(1.82, 0.0, 1.0, 1.4);
    let reach =
        |a: &Animator, foot: &str| a.model[bone(a, foot)].t.z - a.model[bone(a, "hips")].t.z;
    for foot in humanoid::FOOT {
        let (was, now) = (reach(&before, foot), reach(&after, foot));
        assert!(
            (now / was - 1.3).abs() < 0.05,
            "{foot} reached {was:.3} m, now {now:.3} m"
        );
        // Ankles keep their height; the legs stay within reach.
        let (b, a) = (bone(&before, foot), bone(&after, foot));
        assert!(
            (after.model[a].t.y - before.model[b].t.y).abs() < 0.01,
            "{foot} height {} → {}",
            before.model[b].t.y,
            after.model[a].t.y
        );
    }
    let drop = before.model[bone(&before, "hips")].t.y - after.model[bone(&after, "hips")].t.y;
    // (About as much as it takes for the farther foot to reach: a long stride on legs that
    // were nearly straight already.)
    assert!((0.0..0.08).contains(&drop), "hips dropped {drop} m");
    for (upper, lower, foot, _) in legs(&after) {
        let m = &after.model;
        let length = (m[lower].t - m[upper].t).length() + (m[foot].t - m[lower].t).length();
        assert!((m[foot].t - m[upper].t).length() <= length + 1e-3);
    }
}

#[test]
fn standing_or_on_the_fallback_nothing_changes() {
    for (speed, share) in [(0.1, 1.0), (1.4, 0.0)] {
        let (before, after) = warped(speed, 45f32.to_radians(), share, 1.0);
        for (i, (b, a)) in before.local.iter().zip(&after.local).enumerate() {
            assert!(
                (b.t - a.t).length() < 1e-6 && b.r.abs_diff_eq(a.r, 1e-6),
                "speed {speed}, share {share}: bone {i} moved from {b:?} to {a:?}"
            );
        }
    }
}
