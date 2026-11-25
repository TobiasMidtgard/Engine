//! The water's own reflection ray (`trial_cut_more::RAY_WATER`, step 12 part two of the
//! ray-traced lighting round): where a calm body's screen-space march finds nothing, its pixel
//! asks the frame's ray scene what lies along its reflection, and the hit is lit as every
//! pixel's reflection lights its own. Without the lake mirror, a still pool shows a red wall
//! that stands above the screen's top edge with the bit, and the sky without it; beside the
//! wall, where the ray meets nothing, the bit changes nothing. Needs a GPU with ray queries
//! (lavapipe has them); passes with a note without. On lavapipe the ray takes every triangle
//! whole (`ray_scene::water_reflection_lib`): the wall is solid either way.

mod ray_pool;

use engine_core::prelude::*;
use engine_render::RenderAppExt;
use engine_render::settings::trial_cut_more::{RAY_WATER, RAY_WORLD};
use ray_pool::{
    BESIDE, OVER_WALL, POOL_VIEW, Roof, WALL, app, card, colour_at, diag, falls_of, has_gpu,
    look_at, mirrored, picture, stream,
};

#[test]
fn a_pool_reflects_a_wall_that_is_off_screen() {
    let mut shots = Vec::new();
    for (bits, name) in [
        (RAY_WORLD, "pool-plain.png"),
        (RAY_WORLD | RAY_WATER, "pool-rays.png"),
    ] {
        let mut app = app(bits);
        app.update_n(8);
        if !has_gpu(&app) {
            return;
        }
        if diag(&app, "render.ray.supported") != 1.0 {
            eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
            return;
        }
        let traced = bits & RAY_WATER != 0;
        assert_eq!(diag(&app, "render.ray.on"), 1.0, "{name}: no ray scene");
        assert_eq!(diag(&app, "render.ray.losses"), 0.0, "{name}: scene lost");
        assert_eq!(
            diag(&app, "water.ray"),
            if traced { 1.0 } else { 0.0 },
            "{name}: the water's rays"
        );
        assert_eq!(
            diag(&app, "water.ray_bodies"),
            if traced { 1.0 } else { 0.0 },
            "{name}: the calm bodies drawn with them"
        );
        let p = picture(&mut app, name);
        let wall = colour_at(&app, &p, mirrored(WALL));
        let beside = colour_at(&app, &p, mirrored(BESIDE));
        eprintln!("{name}: the wall's reflection {wall:?}, the sky's beside it {beside:?}");
        shots.push((wall, beside));
    }
    let [(plain_wall, plain_beside), (ray_wall, ray_beside)] = shots[..] else {
        unreachable!()
    };
    // The march leaves the screen's top before it meets the wall: the sky without the rays,
    // the red wall with them.
    assert!(
        ray_wall[0] >= plain_wall[0] + 20.0,
        "the wall's red in the pool: {:.1} with the rays, {:.1} without",
        ray_wall[0],
        plain_wall[0]
    );
    assert!(
        ray_wall[2] <= plain_wall[2] + 4.0,
        "the wall's reflection grew blue: {:.1} with the rays, {:.1} without",
        ray_wall[2],
        plain_wall[2]
    );
    // Where the ray meets nothing, the mix is the one without it (the output's dither moves
    // a pixel a level either way).
    for c in 0..3 {
        assert!(
            (ray_beside[c] - plain_beside[c]).abs() <= 2.0,
            "the sky beside the wall changed: {ray_beside:?} with the rays, {plain_beside:?} \
             without"
        );
    }
}

/// The hit's light asks the ray scene too: its shadow ray toward the sun meets a ground's
/// triangles and a cut-out card's as candidates, which the water's shader asks (`ray_first`'s
/// loop, which lavapipe runs in a fragment shader, unlike `ray_nearest`'s:
/// `ray_scene::water_reflection_lib`). A roof of ground over the space in front of the wall,
/// which only the rays see, takes the sun off the wall's face: the pool reflects it dark, and
/// still the wall, not the sky. A card in the roof's place, cut out by its texture, clear over
/// one half and opaque over the other, takes the sun off the half of the wall under its opaque
/// half alone: its texture is read for the candidate.
#[test]
fn the_walls_reflection_is_shaded_by_what_only_the_rays_see_over_it() {
    // The wall's reflection 5 m left of its middle, at it, and 5 m right of it.
    let at = [-5.0, 0.0, 5.0].map(|x| mirrored(WALL + DVec3::new(x, 0.0, 0.0)));
    let mut runs = Vec::new();
    for (over, name) in [
        ("nothing", "pool-sunlit.png"),
        ("roof", "pool-roofed.png"),
        ("card", "pool-card.png"),
    ] {
        let mut app = app(RAY_WORLD | RAY_WATER);
        match over {
            "roof" => {
                app.add_render_feature(Roof);
            }
            "card" => {
                card(&mut app);
            }
            _ => {}
        }
        app.update_n(1);
        if !has_gpu(&app) {
            return;
        }
        if diag(&app, "render.ray.supported") != 1.0 {
            eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
            return;
        }
        if over == "card" {
            // A card's tile is filled from the texture the raster uploads, so the camera first
            // looks up at it.
            look_at(&mut app, OVER_WALL);
            app.update_n(4);
            look_at(&mut app, POOL_VIEW);
        }
        app.update_n(8);
        assert_eq!(
            diag(&app, "water.ray_bodies"),
            1.0,
            "{name}: no calm body traced"
        );
        if over == "card" {
            assert!(
                diag(&app, "render.ray.tiles") >= 1.0
                    && diag(&app, "render.ray.tiles_waiting") == 0.0,
                "the card's texture has no tile: {} tiles, {} waiting",
                diag(&app, "render.ray.tiles"),
                diag(&app, "render.ray.tiles_waiting")
            );
        }
        let p = picture(&mut app, name);
        let walls = at.map(|w| colour_at(&app, &p, w));
        eprintln!("{name}: the wall's reflection 5 m left, at its middle, 5 m right {walls:?}");
        runs.push(walls);
    }
    let [sunlit, roofed, carded] = runs[..] else {
        unreachable!()
    };
    for (i, side) in ["5 m left", "in the middle", "5 m right"]
        .iter()
        .enumerate()
    {
        assert!(
            roofed[i][0] <= sunlit[i][0] - 30.0,
            "{side}: the roofed wall's red in the pool {:.1}, sunlit {:.1}",
            roofed[i][0],
            sunlit[i][0]
        );
        // The ray still meets the wall: the sky's reflection there is bluer by some 20 levels
        // (a_pool_reflects_a_wall_that_is_off_screen's plain picture).
        assert!(
            roofed[i][2] <= sunlit[i][2] + 4.0,
            "{side}: the roofed wall's reflection grew blue: {:.1}, sunlit {:.1}",
            roofed[i][2],
            sunlit[i][2]
        );
    }
    // (Which half of the card is clear follows its uv and its turn.)
    let (open, kept) = if carded[0][0] > carded[2][0] {
        (0, 2)
    } else {
        (2, 0)
    };
    assert!(
        carded[open][0] >= sunlit[open][0] - 10.0,
        "the wall under the card's clear half: red {:.1}, sunlit {:.1}",
        carded[open][0],
        sunlit[open][0]
    );
    assert!(
        carded[kept][0] <= sunlit[kept][0] - 30.0 && carded[kept][2] <= sunlit[kept][2] + 4.0,
        "the wall under the card's opaque half: {:?}, sunlit {:?}",
        carded[kept],
        sunlit[kept]
    );
}

/// The falls' white curtains are in the ray scene (one `NoCast` instance for a stream's
/// sheets, step 12): a stream's fall that hangs above the screen's top edge beside the wall's
/// end shows in the pool with the bit as white water, and the sky without it, as the wall
/// does; beyond the fall's end the sky stays as it was; and the curtains leave with the
/// stream. (Before them the pool showed the sky there with the bit too: the falls were the
/// screen's alone.)
#[test]
fn a_pool_reflects_a_fall_that_is_off_screen() {
    let mut shots = Vec::new();
    for (bits, name) in [
        (RAY_WORLD, "fall-plain.png"),
        (RAY_WORLD | RAY_WATER, "fall-rays.png"),
    ] {
        let mut app = app(bits);
        let region = stream(&mut app);
        app.update_n(1);
        if !has_gpu(&app) {
            return;
        }
        if diag(&app, "render.ray.supported") != 1.0 {
            eprintln!("skipping: the GPU cannot hold the ray scene (no ray queries)");
            return;
        }
        let paths = falls_of(&mut app, region, 240);
        assert!(paths.len() >= 4, "{name}: the stream's falls: {paths:?}");
        app.update_n(8);
        assert_eq!(diag(&app, "render.ray.losses"), 0.0, "{name}: scene lost");
        assert_eq!(
            diag(&app, "render.ray.class_no_cast"),
            1.0,
            "{name}: the stream's curtains"
        );
        // The middle lip's curtain where it has fallen to 9 m, 3 m down: above the screen's top
        // edge, which is about 6 m up there.
        let path = &paths[paths.len() / 2];
        let fall = *path
            .iter()
            .min_by(|a, b| (a.y - 9.0).abs().total_cmp(&(b.y - 9.0).abs()))
            .unwrap();
        let p = picture(&mut app, name);
        let at = colour_at(&app, &p, mirrored(fall));
        let beside = colour_at(&app, &p, mirrored(BESIDE));
        eprintln!(
            "{name}: the fall's reflection at {fall:?}: {at:?}, the sky's beside it {beside:?}"
        );
        shots.push((at, beside));
        if bits & RAY_WATER != 0 {
            app.world.despawn(region);
            app.update_n(3);
            assert_eq!(
                diag(&app, "render.ray.class_no_cast"),
                0.0,
                "the curtains stayed after the stream"
            );
        }
    }
    let [(plain_fall, plain_beside), (ray_fall, ray_beside)] = shots[..] else {
        unreachable!()
    };
    // White where the sky was: as much more red and green as the wall's reflection gains red,
    // and no less blue.
    for (c, colour) in [(0, "red"), (1, "green")] {
        assert!(
            ray_fall[c] >= plain_fall[c] + 15.0,
            "the fall's {colour} in the pool: {:.1} with the rays, {:.1} without",
            ray_fall[c],
            plain_fall[c]
        );
    }
    assert!(
        ray_fall[2] >= plain_fall[2] - 6.0,
        "the fall's reflection lost its blue: {ray_fall:?} with the rays, {plain_fall:?} without"
    );
    for c in 0..3 {
        assert!(
            (ray_beside[c] - plain_beside[c]).abs() <= 2.0,
            "the sky beside the fall changed: {ray_beside:?} with the rays, {plain_beside:?} \
             without"
        );
    }
}
