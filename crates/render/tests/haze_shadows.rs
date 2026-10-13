//! Valley haze in a mountain's shadow doesn't glow: with the sun low behind a ridge 3 km away,
//! the haze between the camera and the ridge's foot lies in the ridge's shadow, so the ridge
//! keeps its contrast instead of hiding behind sunlit haze. Lit from the front nothing changes.
//! Needs a GPU or lavapipe; passes with a note when neither is available.

use engine_core::prelude::*;
use engine_render::prelude::*;
use engine_render::{RenderDisabled, capture_now};

const W: u32 = 160;
const H: u32 = 120;
const EV: f32 = 14.0;

/// Linear radiance (cd/m², mean of the channels) of the ridge's foot and of the sky above it,
/// seen from 5 m above a hazy valley floor (the showcase's haze: 0.00012/m at the floor, 50 m
/// scale height), the sun 20° up `behind` the ridge or behind the camera.
fn render(name: &str, behind: bool, far_shadow_distance: f32) -> Option<(f32, f32)> {
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
        s.volumetric_fog = false;
        s.shadow_distance = 400.0;
        s.far_shadow_distance = far_shadow_distance;
    }
    {
        let mut fog = app.world.resource_mut::<FogSettings>();
        fog.density = 0.00012;
        fog.height_falloff = 0.02;
        fog.base_height = 0.0;
    }
    let w = &mut app.world;
    w.spawn((
        Camera {
            exposure: Exposure::Manual { ev100: EV },
            ..Camera::perspective(30.0)
        },
        Transform::from_xyz(0.0, 5.0, 0.0).looking_at(DVec3::new(0.0, 400.0, -3000.0), Vec3::Y),
    ));
    let elevation = 20f64.to_radians();
    let z = if behind { -1.0 } else { 1.0 };
    let to_sun = DVec3::new(0.15, elevation.sin(), z * elevation.cos()).normalize();
    w.spawn((
        DirectionalLight::default(),
        Transform::default().looking_at(-to_sun, Vec3::Y),
    ));
    w.spawn((
        Transform::default(),
        MeshRenderer::new(
            Shape::Plane {
                size: [16_000.0, 16_000.0],
                subdivisions: 1,
            },
            StandardMaterial::color(0.25, 0.3, 0.15),
        ),
    ));
    // An 800 m ridge, 4 km wide, 3 km away: with the sun behind it, its shadow covers the
    // valley out to about 800 m from the camera.
    w.spawn((
        Transform::from_xyz(0.0, 400.0, -3200.0),
        MeshRenderer::new(
            Shape::Cuboid {
                size: [4000.0, 800.0, 400.0],
            },
            StandardMaterial::color(0.3, 0.3, 0.3),
        ),
    ));
    app.update_n(4);
    if let Some(d) = app.world.get_resource::<RenderDisabled>() {
        eprintln!("skipping: no GPU ({})", d.0);
        return None;
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("haze_{name}.png"));
    capture_now(&mut app.world, &path).expect("screenshot");
    let img = image::open(&path).expect("png").to_rgba8();
    let exposure = 1.0 / (1.2 * 2f32.powf(EV));
    let lin = |v: u8| {
        let c = v as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let mean = |rows: std::ops::Range<u32>| {
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in rows {
            for x in W / 4..W * 3 / 4 {
                let p = img.get_pixel(x, y);
                sum += (lin(p[0]) + lin(p[1]) + lin(p[2])) / 3.0;
                n += 1.0;
            }
        }
        sum / n / exposure
    };
    // Rows at an elevation (degrees) above the camera's horizon: the camera looks 7.5° up at
    // the ridge's middle, its top is 14.8° up and its foot just below the horizon.
    let pitch = (395f32 / 3000.0).atan();
    let tan_half = (15f32).to_radians().tan();
    let row =
        |deg: f32| ((0.5 - (deg.to_radians() - pitch).tan() / tan_half * 0.5) * H as f32) as u32;
    let foot = mean(row(1.5)..row(0.3));
    let sky = mean(row(22.0)..row(17.0));
    Some((foot, sky))
}

#[test]
fn haze_in_a_ridges_shadow_does_not_glow() {
    let Some((foot_on, sky_on)) = render("backlit_on", true, 8000.0) else {
        return;
    };
    let (foot_off, sky_off) = render("backlit_off", true, 0.0).unwrap();
    let (front_on, _) = render("frontlit_on", false, 8000.0).unwrap();
    let (front_off, _) = render("frontlit_off", false, 0.0).unwrap();
    eprintln!(
        "ridge foot, backlit: {foot_on:.0} cd/m² with haze shadows, {foot_off:.0} without (sky {sky_on:.0} / {sky_off:.0}); front-lit: {front_on:.0} / {front_off:.0}"
    );
    assert!(
        foot_on < foot_off * 0.7,
        "the haze in the ridge's shadow still glows: {foot_on:.0} vs {foot_off:.0} cd/m²"
    );
    assert!(
        (sky_on - sky_off).abs() < sky_off * 0.02,
        "the sky changed: {sky_on:.0} vs {sky_off:.0}"
    );
    assert!(
        (front_on - front_off).abs() < front_off * 0.05,
        "a front-lit ridge's haze changed: {front_on:.0} vs {front_off:.0}"
    );
}
