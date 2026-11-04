//! `ShadowTexelRange`: a caster limited to the fine sun cascades and a `ShadowOnly` stand-in
//! limited to the coarse ones cast exactly once into each cascade, and together they cast the
//! same shadows as the caster alone. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 192;
const H: u32 = 108;
/// Between the first cascade's texels (millimeters) and the last one's (tens of centimeters).
const TEXEL: f32 = 0.05;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Casters {
    /// Pillars with no range: every cascade.
    Plain,
    /// Only the cascades finer than `TEXEL`.
    Fine,
    /// Only those from `TEXEL` on.
    Coarse,
    /// Fine pillars, each with a coarse `ShadowOnly` stand-in of the same shape.
    Pair,
}

/// Pillars down a lane from 4 m to 150 m, under a low sun, without static shadow images (so
/// every cascade draws all its casters every frame): the cascade instances drawn and the shot.
fn shoot(casters: Casters) -> Option<(f64, image::RgbImage)> {
    engine_core::log::init("error");
    let mut app = App::new();
    app.world.resource_mut::<EngineConfig>().headless = true;
    {
        let mut w = app.world.resource_mut::<Window>();
        w.width = W;
        w.height = H;
    }
    app.add_plugins(RenderPlugin);
    {
        let mut s = app.world.resource_mut::<RenderSettings>();
        s.anti_aliasing = AntiAliasing::None;
        s.film_grain = 0.0;
        s.shadow_cascade_reuse = 0;
    }
    let w = &mut app.world;
    w.spawn((
        Camera::perspective(50.0),
        Transform::from_xyz(0.0, 3.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -60.0), Vec3::Y),
    ));
    let e = 25f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default()
            .looking_at(DVec3::new(0.8 * e.cos(), -e.sin(), -0.6 * e.cos()), Vec3::Y),
    ));
    // The ground only receives: the cascades' instances are the pillars'.
    w.spawn((
        Transform::default(),
        MeshRenderer {
            cast_shadows: false,
            ..MeshRenderer::new(
                Shape::Plane {
                    size: [800.0, 800.0],
                    subdivisions: 1,
                },
                StandardMaterial::color(0.5, 0.5, 0.45),
            )
        },
    ));
    let pillar = |size: f32| Shape::Cuboid {
        size: [size, 4.0 * size, size],
    };
    let material = StandardMaterial::color(0.6, 0.55, 0.5);
    for (i, z) in [4.0, 9.0, 18.0, 35.0, 70.0, 150.0].into_iter().enumerate() {
        let size = 0.5 + z as f32 / 40.0;
        let at = Transform::from_xyz(
            if i % 2 == 0 { -1.5 } else { 1.5 } * (1.0 + z / 20.0),
            2.0 * size as f64,
            -z,
        );
        let mut e = w.spawn((at, MeshRenderer::new(pillar(size), material)));
        match casters {
            Casters::Plain => {}
            Casters::Fine | Casters::Pair => {
                e.insert(ShadowTexelRange::finer_than(TEXEL));
            }
            Casters::Coarse => {
                e.insert(ShadowTexelRange::from_texels(TEXEL));
            }
        }
        if casters == Casters::Pair {
            w.spawn((
                at,
                MeshRenderer::new(pillar(size), material),
                ShadowOnly,
                ShadowTexelRange::from_texels(TEXEL),
            ));
        }
    }
    app.update_n(30);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let instances = app
        .world
        .resource::<Diagnostics>()
        .get("render.meshes.cascade_instances")
        .unwrap_or(0.0);
    let shot = format!("texel_range_{casters:?}.png").to_lowercase();
    let dir = std::env::temp_dir().join("engine-shadow-texel-range");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(&shot);
    capture_now(&mut app.world, &path).expect("screenshot");
    if let Ok(out) = std::env::var("ENGINE_SHOTS") {
        let _ = std::fs::copy(&path, std::path::Path::new(&out).join(&shot));
    }
    Some((
        instances,
        image::open(&path).expect("read the shot").to_rgb8(),
    ))
}

/// Pixels with a channel more than 8 apart.
fn differing(a: &image::RgbImage, b: &image::RgbImage) -> usize {
    a.pixels()
        .zip(b.pixels())
        .filter(|(p, q)| p.0.iter().zip(q.0).any(|(&x, y)| x.abs_diff(y) > 8))
        .count()
}

#[test]
fn a_stand_in_for_the_coarse_cascades_casts_once_in_each_and_the_same_shadows() {
    let Some((plain, plain_shot)) = shoot(Casters::Plain) else {
        return;
    };
    let (fine, fine_shot) = shoot(Casters::Fine).expect("GPU");
    let (coarse, _) = shoot(Casters::Coarse).expect("GPU");
    let (pair, pair_shot) = shoot(Casters::Pair).expect("GPU");
    let lost = differing(&plain_shot, &fine_shot);
    let changed = differing(&plain_shot, &pair_shot);
    eprintln!(
        "cascade instances: plain {plain}, fine {fine}, coarse {coarse}, pair {pair}; \
         fine alone changes {lost} pixels, the pair {changed}"
    );
    // Each pillar is in some fine cascades and some coarse ones; the two ranges split them.
    assert!(fine > 0.0 && coarse > 0.0, "fine {fine}, coarse {coarse}");
    assert_eq!(fine + coarse, plain, "the ranges overlap or leave a gap");
    // With its stand-in, each map still draws one of the two.
    assert_eq!(
        pair, plain,
        "the pair casts {pair} times, the pillars {plain}"
    );
    // The fine range alone loses the far pillars' shadows; the stand-in brings them back.
    assert!(
        lost > 20,
        "the coarse cascades drew the fine casters: {lost} pixels changed"
    );
    let total = (W * H) as usize;
    assert!(
        changed * 1000 <= total,
        "the pair's shadows differ from the pillars': {changed} of {total} pixels"
    );
}
