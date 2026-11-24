//! The water's reflection ray (`trial_cut_more::RAY_WATER`) on a GPU without ray queries
//! (`ENGINE_RAY_QUERIES=off` leaves the device without them, as a GPU that has none is): no
//! frame's ray scene is made, the water never builds its traced pipeline and draws the pool of
//! ray_water.rs as it does without the bit. A test binary of its own: the variable is the whole
//! process's, read when the device is made. Needs a GPU or lavapipe; passes with a note when
//! neither is available.

mod ray_pool;

use engine_render::settings::trial_cut_more::{RAY_WATER, RAY_WORLD};
use ray_pool::{H, W, app, diag, has_gpu, picture};

#[test]
fn without_ray_queries_the_waters_rays_change_nothing() {
    // SAFETY: the only test of this binary, before any other thread exists.
    unsafe { std::env::set_var("ENGINE_RAY_QUERIES", "off") };
    let mut shots = Vec::new();
    for (bits, name) in [
        (RAY_WORLD, "pool-off-plain.png"),
        (RAY_WORLD | RAY_WATER, "pool-off-rays.png"),
    ] {
        let mut app = app(bits);
        app.update_n(8);
        if !has_gpu(&app) {
            return;
        }
        for (key, expected) in [
            ("render.ray_queries", 0.0),
            ("render.ray.supported", 0.0),
            ("render.ray.on", 0.0),
            ("water.ray", 0.0),
            ("water.ray_bodies", 0.0),
        ] {
            assert_eq!(diag(&app, key), expected, "{name}: {key}");
        }
        shots.push(picture(&mut app, name));
    }
    // Two runs of one scene differ by the output's dither alone (a level either way).
    let (mut over, mut most) = (0u32, 0u8);
    for (a, b) in shots[0].pixels().zip(shots[1].pixels()) {
        let d = (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0);
        over += (d > 2) as u32;
        most = most.max(d);
    }
    assert!(
        over <= W * H / 1000 && most <= 8,
        "the bit changed the picture: {over} pixels apart by more than the dither, at most \
         {most}"
    );
}
