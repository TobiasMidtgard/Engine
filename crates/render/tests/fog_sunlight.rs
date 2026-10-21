//! Sunlight inside a fog bank: deep in thick ground fog the sun's beam fades along its path
//! through the fog above, so shadows soften and the ground is lit by the fog around instead;
//! in thin haze nothing changes. Needs a GPU or lavapipe; passes with a note when neither is
//! available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 120;

/// Mean linear radiance (sRGB decoded, mean of the channels) of the sunlit ground and of the
/// ground in a low wall's shadow, seen from 4 m looking down, the sun 40° up, under height
/// fog of `density` per meter at the ground thinning over a 50 m scale height.
fn lit_and_shadowed(name: &str, density: f32) -> Option<(f32, f32)> {
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
        s.tonemapper = Tonemapper::None;
        s.bloom_intensity = 0.0;
        s.vignette = 0.0;
        s.film_grain = 0.0;
        s.anti_aliasing = AntiAliasing::None;
    }
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = density;
        fog.height_falloff = 0.02;
        fog.base_height = 0.0;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: 14.0 },
            ..Camera::perspective(60.0)
        },
        Transform::from_xyz(0.0, 4.0, 0.0).looking_at(DVec3::new(0.0, 0.0, -0.5), Vec3::Y),
    ));
    // The sun 40° up to the left (-x), so the 1 m wall at x = 0 shades the ground from x = 0
    // to 1.2 m.
    let e = 40f64.to_radians();
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(DVec3::new(e.cos(), -e.sin(), 0.0), Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [100.0, 100.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
    ));
    w.spawn((
        Transform::from_xyz(0.0, 0.5, 0.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [0.1, 1.0, 20.0],
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
    ));
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path =
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fog_sun_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let decode = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let mean = |x0: u32, x1: u32| {
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in (H / 2 - 15)..(H / 2 + 15) {
            for x in x0..x1 {
                let p = img.get_pixel(x, y);
                sum += (decode(p[0]) + decode(p[1]) + decode(p[2])) / 3.0;
                n += 1.0;
            }
        }
        sum / n
    };
    // The ground 1.1-1.7 m left of the wall, and 0.1-0.9 m right of it, in its shadow.
    Some((mean(15, 45), mean(W / 2 + 4, W / 2 + 24)))
}

#[test]
fn shadows_soften_deep_in_a_fog_bank() {
    let Some((lit, shade)) = lit_and_shadowed("haze", 0.0001) else {
        return;
    };
    let (fog_lit, fog_shade) = lit_and_shadowed("fog", 0.02).unwrap();
    eprintln!("haze: lit {lit:.4} shade {shade:.4}; fog: lit {fog_lit:.4} shade {fog_shade:.4}");
    // In thin haze the sun keeps its strength: a hard shadow.
    assert!(
        lit > shade * 4.0,
        "no sun shadow in thin haze: {lit} vs {shade}"
    );
    // Under one optical depth of fog (0.02 per meter over a 50 m scale height) about a fifth of
    // the beam gets through at 40°; most of the rest reaches the ground scattered, from the fog
    // around, and lights the shade too: a faint shadow.
    assert!(
        fog_lit < lit * 0.9,
        "the fog doesn't dim the sun: {fog_lit} vs {lit}"
    );
    assert!(
        fog_lit < fog_shade * 1.5,
        "the shadow stays hard in the fog: {fog_lit} vs {fog_shade}"
    );
    assert!(
        fog_shade > shade,
        "the fog doesn't light the shade: {fog_shade} vs {shade}"
    );
}
